// SPDX-License-Identifier: AGPL-3.0-or-later
// A done adoption's report (`WriteState.done.report`) is shown for its acting account, once
// (engine-api.md §3.5 `mod_adopt`; review finding sec-8).
import { MockEngine } from "@lessdugin/engine/mock";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { qk } from "../engine/queries.ts";
import { WriteStore } from "../engine/writes.ts";
import { renderWithApp } from "../test/render.tsx";
import { AdoptionReport } from "./AdoptionReport.tsx";

const NS = "online.7778777.lessdugin.temp";
const MOD = `at://did:plc:moderatormoderatormoderat/${NS}.modAction`;
const LINES = [`cannot adopt ${MOD}/k-1: not in effect`, `kept own assertion for ${MOD}/k-2`];

describe("AdoptionReport", () => {
  it("lists what a done adoption did not adopt, once per write", async () => {
    const store = new WriteStore();
    const me = (await new MockEngine().activeAccount())?.did;
    if (!me) throw new Error("no active account");
    const id = `${me}/8`;
    renderWithApp(<AdoptionReport store={store} />);
    expect(screen.queryByText("Adoption report")).toBeNull();
    // a write that is done without a report (everything adopted, or not an adoption) says nothing
    store.update(`${me}/7`, { kind: "done", uri: `${MOD}/k-0` });
    store.update(id, { kind: "sending" });
    expect(screen.queryByText("Adoption report")).toBeNull();
    store.update(id, { kind: "done", uri: `${MOD}/a-1`, report: LINES });
    expect(await screen.findByText("Adoption report")).toBeTruthy();
    expect(screen.getByText(/; 2 assertions not adopted/)).toBeTruthy();
    const items = within(screen.getByTestId("adoption-report")).getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual(LINES);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByText("Adoption report")).toBeNull());
    // the same done state again (an event replayed, an outbox listing) does not reopen it
    store.update(id, { kind: "done", uri: `${MOD}/a-1`, report: LINES });
    expect(screen.queryByText("Adoption report")).toBeNull();
  });

  it("keeps A's report hidden after switching to B and recovers it on returning to A", async () => {
    const engine = new MockEngine();
    const store = new WriteStore();
    const a = (await engine.activeAccount())?.did;
    if (!a) throw new Error("no active account");
    const b = "did:plc:accountbbbbbbbbbbbbbbbbbbbb";
    await engine.addAccount({ did: b, pds: "https://pds.example.test", auth: "appPassword" });
    const { queryClient } = renderWithApp(<AdoptionReport store={store} />, { engine });
    // Startup seeding can contain either account's retained writes, but presentation is scoped.
    store.seed([
      { id: `${b}/20`, state: { kind: "done", uri: `${MOD}/b`, report: ["B report"] } },
      { id: `${a}/20`, state: { kind: "done", uri: `${MOD}/a`, report: ["A report"] } },
    ]);
    expect((await screen.findByTestId("adoption-report")).textContent).toContain("A report");
    await act(async () => {
      await engine.setActiveAccount(b);
      await queryClient.invalidateQueries();
    });
    await screen.findByText("B report");
    store.update(`${a}/20`, { kind: "done", uri: `${MOD}/a`, report: ["A report"] });
    expect(screen.queryByText("A report")).toBeNull();
    expect(store.pendingReport(a)?.lines).toEqual(["A report"]);
    expect(store.pendingReport(b)?.lines).toEqual(["B report"]);
    await act(async () => {
      await engine.setActiveAccount(a);
      await queryClient.invalidateQueries();
    });
    expect((await screen.findByTestId("adoption-report")).textContent).toContain("A report");
  });

  it("hides stale A data after a failed switch to B, then recovers B and A queues", async () => {
    const engine = new MockEngine();
    const store = new WriteStore();
    const a = (await engine.activeAccount())?.did;
    if (!a) throw new Error("no active account");
    const b = "did:plc:accountbbbbbbbbbbbbbbbbbbbb";
    await engine.addAccount({ did: b, pds: "https://pds.example.test", auth: "appPassword" });
    const { queryClient } = renderWithApp(<AdoptionReport store={store} />, { engine });
    store.seed([
      { id: `${a}/30`, state: { kind: "done", uri: `${MOD}/a`, report: ["A report"] } },
      { id: `${b}/30`, state: { kind: "done", uri: `${MOD}/b`, report: ["B report"] } },
    ]);
    expect((await screen.findByTestId("adoption-report")).textContent).toContain("A report");

    const original = engine.activeAccount.bind(engine);
    let fail = false;
    engine.activeAccount = async () => {
      if (fail) throw new Error("active account refresh failed");
      return original();
    };
    fail = true;
    await act(async () => {
      await engine.setActiveAccount(b);
      await queryClient.invalidateQueries();
    });
    await waitFor(() =>
      expect(queryClient.getQueryState(qk.activeAccount())?.status).toBe("error"),
    );
    // TanStack Query retains the previous data on a failed refetch, as it does with
    // placeholderData/keepPreviousData. It must not be treated as B's resolved identity.
    expect(queryClient.getQueryData<{ did: string }>(qk.activeAccount())?.did).toBe(a);
    expect(screen.queryByText("A report")).toBeNull();
    expect(screen.queryByText("B report")).toBeNull();
    expect(store.pendingReport(a)?.lines).toEqual(["A report"]);
    expect(store.pendingReport(b)?.lines).toEqual(["B report"]);

    fail = false;
    await act(async () => {
      await queryClient.invalidateQueries();
    });
    expect((await screen.findByTestId("adoption-report")).textContent).toContain("B report");
    expect(screen.queryByText("A report")).toBeNull();

    await act(async () => {
      await engine.setActiveAccount(a);
      await queryClient.invalidateQueries();
    });
    expect((await screen.findByTestId("adoption-report")).textContent).toContain("A report");
    expect(screen.queryByText("B report")).toBeNull();
    expect(store.pendingReport(a)?.lines).toEqual(["A report"]);
    expect(store.pendingReport(b)?.lines).toEqual(["B report"]);
  });

  it("shows no report while the active account query is unresolved", async () => {
    const engine = new MockEngine();
    const a = (await engine.activeAccount())?.did;
    if (!a) throw new Error("no active account");
    const store = new WriteStore();
    store.seed([{ id: `${a}/20`, state: { kind: "done", uri: `${MOD}/a`, report: ["A report"] } }]);
    let answer: (() => void) | undefined;
    const pending = new Promise<void>((resolve) => {
      answer = resolve;
    });
    const original = engine.activeAccount.bind(engine);
    let hold = false;
    engine.activeAccount = async () => {
      if (hold) await pending;
      return original();
    };
    hold = true;
    const { queryClient } = renderWithApp(<AdoptionReport store={store} />, { engine });
    expect(screen.queryByText("Adoption report")).toBeNull();
    await act(async () => {
      answer?.();
      await queryClient.invalidateQueries();
    });
    expect(await screen.findByText("Adoption report")).toBeTruthy();
  });

  it("shows reports in turn", async () => {
    const store = new WriteStore();
    const me = (await new MockEngine().activeAccount())?.did;
    if (!me) throw new Error("no active account");
    const one = `${me}/1`;
    const two = `${me}/2`;
    renderWithApp(<AdoptionReport store={store} />);
    store.update(one, { kind: "done", uri: `${MOD}/a-1`, report: [LINES[0] as string] });
    store.update(two, { kind: "done", uri: `${MOD}/a-2`, report: [LINES[1] as string] });
    expect((await screen.findByText(new RegExp(`\\(write ${one}\\)`))).textContent).toMatch(
      /1 assertion not/,
    );
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(await screen.findByText(new RegExp(`\\(write ${two}\\)`))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByText("Adoption report")).toBeNull());
  });
});
