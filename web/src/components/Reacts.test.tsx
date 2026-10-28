// SPDX-License-Identifier: AGPL-3.0-or-later
import { MockEngine } from "@lessdugin/engine/mock";
import type { PostSummary, ReactTally } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { QuoteSelection } from "../lib/quote.ts";
import { renderWithApp } from "../test/render.tsx";
import { ReactBar } from "./Reacts.tsx";

async function otherPost(engine: MockEngine): Promise<PostSummary> {
  const forum = (await engine.followed())[0]?.uri ?? "";
  const me = (await engine.activeAccount())?.did;
  const list = await engine.listPosts({ forum, sort: "top", filter: "all", limit: 50 });
  const post = list.items.find((p) => p.author.did !== me);
  if (!post) throw new Error("no post by someone else");
  return post;
}

/** Presses the trigger the way a pointer does: pointerdown, then click. */
function press(el: HTMLElement): void {
  fireEvent.pointerDown(el);
  fireEvent.click(el);
}

describe("ReactBar palette", () => {
  it("opens and closes from its trigger, and closes on an outside press", async () => {
    const engine = new MockEngine();
    const post = await otherPost(engine);
    renderWithApp(
      <>
        <ReactBar subject={post.uri} reacts={[]} getQuote={() => null} />
        <p>outside</p>
      </>,
      { engine },
    );
    const trigger = await screen.findByRole("button", { name: "Add react" });
    press(trigger);
    expect(screen.getByRole("dialog", { name: "Add a react" })).toBeTruthy();
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    // The trigger closes the palette it opened (it used to reopen it at once).
    press(trigger);
    expect(screen.queryByRole("dialog", { name: "Add a react" })).toBeNull();
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    press(trigger);
    expect(screen.getByRole("dialog", { name: "Add a react" })).toBeTruthy();
    fireEvent.pointerDown(screen.getByText("outside"));
    expect(screen.queryByRole("dialog", { name: "Add a react" })).toBeNull();
  });

  it("warns when the selection cannot be anchored and reacts to the whole item", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const post = await otherPost(engine);
    const opinion = vi.spyOn(engine, "opinion");
    const unmatched: QuoteSelection = { kind: "unmatched", exact: "x y" };
    renderWithApp(<ReactBar subject={post.uri} reacts={[]} getQuote={() => unmatched} />, {
      engine,
    });
    press(await screen.findByRole("button", { name: "Add react" }));
    const palette = screen.getByRole("dialog", { name: "Add a react" });
    expect(within(palette).getByRole("status").textContent).toMatch(/cannot be anchored/);
    fireEvent.click(within(palette).getAllByRole("button")[1] as HTMLElement);
    const ack = await screen.findByRole("dialog", { name: "Reacts are public" });
    fireEvent.click(within(ack).getByRole("button", { name: "I understand, continue" }));
    await waitFor(() => expect(opinion).toHaveBeenCalledTimes(1));
    const intent = opinion.mock.calls[0]?.[1];
    expect(intent?.kind).toBe("react");
    expect(intent && "quote" in intent).toBe(false);
  });

  it("sends an anchored selection as the react's quote", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const post = await otherPost(engine);
    const opinion = vi.spyOn(engine, "opinion");
    const anchored: QuoteSelection = {
      kind: "anchored",
      quote: { exact: "same view", start: 3, prefix: "the " },
    };
    renderWithApp(<ReactBar subject={post.uri} reacts={[]} getQuote={() => anchored} />, {
      engine,
    });
    press(await screen.findByRole("button", { name: "Add react" }));
    const palette = screen.getByRole("dialog", { name: "Add a react" });
    expect(palette.textContent).toContain("same view");
    fireEvent.click(within(palette).getAllByRole("button")[1] as HTMLElement);
    const ack = await screen.findByRole("dialog", { name: "Reacts are public" });
    fireEvent.click(within(ack).getByRole("button", { name: "I understand, continue" }));
    await waitFor(() => expect(opinion).toHaveBeenCalledTimes(1));
    expect(opinion.mock.calls[0]?.[1]).toMatchObject({ kind: "react", quote: anchored.quote });
  });
});

describe("ReactBar tallies", () => {
  const tally = (over: Partial<ReactTally> = {}): ReactTally => ({
    react: "insightful",
    label: "Insightful",
    glyph: "💡",
    for: 1,
    against: 0,
    creator: "did:plc:abcdefghijklmnopqrstuvwx",
    ...over,
  });

  it("names a creator by verified handle or short DID, never by display name (sec-11)", async () => {
    const engine = new MockEngine();
    const post = await otherPost(engine);
    vi.spyOn(engine, "resolveActor").mockResolvedValue({
      did: "did:plc:abcdefghijklmnopqrstuvwx",
      shortDid: "did:plc:abcd…uvwx",
      handleState: "unverified",
      displayName: "@alice.bsky.social",
    });
    renderWithApp(<ReactBar subject={post.uri} reacts={[tally()]} getQuote={() => null} />, {
      engine,
    });
    const button = await screen.findByRole("button", { name: /^Insightful/ });
    await waitFor(() => expect(button.title).toContain("added by did:plc:abcd…uvwx"));
    expect(button.textContent).not.toContain("alice");
  });

  it("toggling on and off within the hold shows the server's count again (m3r-11)", async () => {
    // a hold longer than the checks below: the counts are read while both writes are held
    const engine = new MockEngine({ holdMs: 2500, sendMs: 0 });
    const post = await otherPost(engine);
    const reacts = [tally()];
    renderWithApp(<ReactBar subject={post.uri} reacts={reacts} getQuote={() => null} />, {
      engine,
    });
    const tallyButton = (): HTMLElement => screen.getByRole("button", { name: /^Insightful/ });
    // the button's third span is the `for` count
    const shownFor = (): string | null | undefined =>
      tallyButton().querySelectorAll(":scope > span")[2]?.textContent;
    fireEvent.click(await screen.findByRole("button", { name: /^Insightful/ }));
    const ack = await screen.findByRole("dialog", { name: "Reacts are public" });
    fireEvent.click(within(ack).getByRole("button", { name: "I understand, continue" }));
    await waitFor(() => expect(tallyButton().getAttribute("aria-pressed")).toBe("true"));
    expect(shownFor()).toBe("2");
    fireEvent.click(tallyButton());
    // Still within the hold (the pending badge is up): the delta is taken against the server's
    // stance, so the tally shows the server's 1. Against the shown stance it would be 0 and
    // the tally would leave the list until the writes settled.
    await waitFor(() => expect(tallyButton().getAttribute("aria-pressed")).toBe("false"), {
      timeout: 1000,
    });
    expect(shownFor()).toBe("1");
    expect(screen.getByText("[pending]")).toBeTruthy();
    // the writes settle with nothing changed, and `reacts` stays the same array: the pending
    // badge still goes
    await waitFor(() => expect(screen.queryByText("[pending]")).toBeNull(), { timeout: 6000 });
    expect(shownFor()).toBe("1");
  }, 10_000);
});
