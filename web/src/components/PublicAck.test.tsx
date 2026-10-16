// SPDX-License-Identifier: AGPL-3.0-or-later
import type { PublicActionKind } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { usePublicAck } from "./PublicAck.tsx";

/** Buttons that each request an acknowledgement and record how the request settled. */
function Requester({ results }: { results: Record<string, boolean[]> }) {
  const request = usePublicAck();
  const ask = (kind: PublicActionKind): void => {
    void request(kind).then((ok) => {
      results[kind] = [...(results[kind] ?? []), ok];
    });
  };
  return (
    <>
      <button type="button" onClick={() => ask("vote")}>
        vote
      </button>
      <button type="button" onClick={() => ask("react")}>
        react
      </button>
    </>
  );
}

const continueIn = (name: string) =>
  screen
    .findByRole("dialog", { name })
    .then((d) => within(d).getByRole("button", { name: "I understand, continue" }));

describe("PublicAck (social.md §17)", () => {
  it("queues concurrent requests: one dialog at a time, and every request settles", async () => {
    const results: Record<string, boolean[]> = {};
    renderWithApp(<Requester results={results} />);
    fireEvent.click(await screen.findByRole("button", { name: "vote" }));
    fireEvent.click(screen.getByRole("button", { name: "react" }));
    fireEvent.click(screen.getByRole("button", { name: "vote" }));
    // The first dialog answers both vote requests…
    fireEvent.click(await continueIn("Votes are public"));
    await waitFor(() => expect(results.vote).toEqual([true, true]));
    // …then the react request gets its own dialog.
    const react = await screen.findByRole("dialog", { name: "Reacts are public" });
    fireEvent.click(within(react).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(results.react).toEqual([false]));
    expect(screen.queryByRole("dialog")).toBeNull();
    // Acknowledged kinds pass without asking.
    fireEvent.click(screen.getByRole("button", { name: "vote" }));
    await waitFor(() => expect(results.vote).toEqual([true, true, true]));
  });

  it("does not let a double click answer the next queued request", async () => {
    const results: Record<string, boolean[]> = {};
    renderWithApp(<Requester results={results} />);
    fireEvent.click(await screen.findByRole("button", { name: "vote" }));
    fireEvent.click(screen.getByRole("button", { name: "react" }));
    const yes = await continueIn("Votes are public");
    fireEvent.click(yes);
    fireEvent.click(yes);
    await waitFor(() => expect(results.vote).toEqual([true]));
    // The react request is still waiting for its own answer.
    expect(await screen.findByRole("dialog", { name: "Reacts are public" })).toBeTruthy();
    expect(results.react).toBeUndefined();
  });
});
