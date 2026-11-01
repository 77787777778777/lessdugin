// SPDX-License-Identifier: AGPL-3.0-or-later
// A done adoption's report (`WriteState.done.report`) is shown when its write event arrives,
// once (engine-api.md §3.5 `mod_adopt`; review finding sec-8).
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { WriteStore } from "../engine/writes.ts";
import { AdoptionReport } from "./AdoptionReport.tsx";

const NS = "online.7778777.lessdugin.temp";
const MOD = `at://did:plc:moderatormoderatormoderat/${NS}.modAction`;
const LINES = [`cannot adopt ${MOD}/k-1: not in effect`, `kept own assertion for ${MOD}/k-2`];

describe("AdoptionReport", () => {
  it("lists what a done adoption did not adopt, once per write", async () => {
    const store = new WriteStore();
    render(<AdoptionReport store={store} />);
    expect(screen.queryByText("Adoption report")).toBeNull();
    // a write that is done without a report (everything adopted, or not an adoption) says nothing
    store.update("w7", { kind: "done", uri: `${MOD}/k-0` });
    store.update("w8", { kind: "sending" });
    expect(screen.queryByText("Adoption report")).toBeNull();
    store.update("w8", { kind: "done", uri: `${MOD}/a-1`, report: LINES });
    expect(await screen.findByText("Adoption report")).toBeTruthy();
    expect(screen.getByText(/\(write w8\); 2 assertions not adopted/)).toBeTruthy();
    const items = within(screen.getByTestId("adoption-report")).getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual(LINES);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByText("Adoption report")).toBeNull());
    // the same done state again (an event replayed, an outbox listing) does not reopen it
    store.update("w8", { kind: "done", uri: `${MOD}/a-1`, report: LINES });
    expect(screen.queryByText("Adoption report")).toBeNull();
  });

  it("shows reports in turn", async () => {
    const store = new WriteStore();
    render(<AdoptionReport store={store} />);
    store.update("w1", { kind: "done", uri: `${MOD}/a-1`, report: [LINES[0] as string] });
    store.update("w2", { kind: "done", uri: `${MOD}/a-2`, report: [LINES[1] as string] });
    expect((await screen.findByText(/\(write w1\)/)).textContent).toMatch(/1 assertion not/);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(await screen.findByText(/\(write w2\)/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByText("Adoption report")).toBeNull());
  });
});
