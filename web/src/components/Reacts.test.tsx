// SPDX-License-Identifier: AGPL-3.0-or-later
import { MockEngine } from "@lessdugin/engine/mock";
import type { PostSummary } from "@lessdugin/engine/types";
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
