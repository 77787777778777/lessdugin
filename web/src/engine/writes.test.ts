// SPDX-License-Identifier: AGPL-3.0-or-later
// The write store: seeding from `writes()`, and `settle`, which lets a composer go once a write
// waits in the outbox instead of spinning until it is sent (robustness review H3).

import type { WriteTicket } from "@lessdugin/engine/types";
import { describe, expect, it } from "vitest";
import { canSendNow, QUEUED_AFTER_MS, WriteStore, waitingText } from "./writes.ts";

const NOW = 1_000_000;
const A = "did:plc:accountaaaaaaaaaaaaaaaaaaaa";
const B = "did:plc:accountbbbbbbbbbbbbbbbbbbbb";
const ticket = (id: string, state: WriteTicket["state"]): WriteTicket => ({ id, state });
const reported = (id: string, line = "not adopted: private markdown must not be stored") =>
  ticket(id, { kind: "done", uri: "at://x", report: [line] });

describe("WriteStore", () => {
  it("settles with the final state when the write finishes", async () => {
    const s = new WriteStore();
    s.track({ id: "w1", state: { kind: "pending" } });
    const p = s.settle("w1", QUEUED_AFTER_MS, () => NOW);
    s.update("w1", { kind: "sending" });
    s.update("w1", { kind: "done", uri: "at://x" });
    await expect(p).resolves.toEqual({ kind: "done", uri: "at://x" });
  });

  it("settles as queued as soon as the write waits for a back-off or a rate limit", async () => {
    const s = new WriteStore();
    s.track({ id: "w1", state: { kind: "pending" } });
    const p = s.settle("w1", 60_000, () => NOW);
    s.update("w1", { kind: "sending" });
    s.update("w1", { kind: "pending", notBeforeMs: NOW + 30_000, throttled: true });
    await expect(p).resolves.toEqual({
      kind: "queued",
      state: { kind: "pending", notBeforeMs: NOW + 30_000, throttled: true },
    });
    // already waiting when asked
    await expect(s.settle("w1", 60_000, () => NOW)).resolves.toMatchObject({ kind: "queued" });
  });

  it("settles as queued when the account's session was refused (needsLogin)", async () => {
    const s = new WriteStore();
    s.track({ id: "w1", state: { kind: "pending" } });
    const p = s.settle("w1", 60_000, () => NOW);
    const state = {
      kind: "pending" as const,
      needsLogin: true,
      error: { code: "auth_required" as const, message: "expired", retryable: true },
    };
    s.update("w1", state);
    await expect(p).resolves.toEqual({ kind: "queued", state });
    expect(waitingText(state, NOW)).toMatch(/after you sign in/);
    expect(canSendNow(state)).toBe(false);
    expect(
      canSendNow({ kind: "pending", throttled: true, error: { ...state.error, code: "network" } }),
    ).toBe(true);
    expect(canSendNow({ kind: "pending", throttled: true })).toBe(false);
  });

  it("does not let go during the undo window, but does after the grace period", async () => {
    const s = new WriteStore();
    s.track({ id: "w1", state: { kind: "pending", held: true, notBeforeMs: NOW + 5000 } });
    const p = s.settle("w1", 20, () => NOW);
    await expect(p).resolves.toEqual({
      kind: "queued",
      state: { kind: "pending", held: true, notBeforeMs: NOW + 5000 },
    });
    s.track({ id: "w2", state: { kind: "sending" } });
    await expect(s.settle("w2", 20, () => NOW)).resolves.toEqual({
      kind: "queued",
      state: { kind: "sending" },
    });
  });

  it("seeds writes queued before the page loaded without overriding events", () => {
    const s = new WriteStore();
    s.update("w1", { kind: "sending" });
    s.seed([
      { id: "w1", state: { kind: "pending" } },
      {
        id: "w2",
        state: { kind: "failed", error: { code: "network", message: "down", retryable: true } },
      },
      { id: "w3", state: { kind: "done", uri: "at://x" } },
    ]);
    expect(s.get("w1")).toEqual({ kind: "sending" });
    expect(s.unfinished().map((w) => w.id)).toEqual(["w1", "w2"]);
    expect(s.failedCount()).toBe(1);
    expect(s.inFlight()).toBe(1);
  });

  it.each(["pending", "sending"] as const)(
    "reconciles a done report after a %s ticket or event without losing it (sec-8)",
    (initial) => {
      const id = `${A}/41`;
      for (const arrive of ["ticket", "event"] as const) {
        const s = new WriteStore();
        if (arrive === "ticket") s.track(ticket(id, { kind: initial }));
        else s.update(id, { kind: initial });
        s.seed([reported(id)]);
        expect(s.get(id)?.kind).toBe("done");
        expect(s.pendingReport(A)).toEqual({
          id,
          lines: ["not adopted: private markdown must not be stored"],
        });
        // A delayed outbox snapshot cannot regress the terminal state or replay the report.
        s.seed([ticket(id, { kind: initial })]);
        expect(s.get(id)?.kind).toBe("done");
        s.dismissReport(id);
        expect(s.pendingReport(A)).toBeUndefined();
        expect(s.pendingReport(B)).toBeUndefined();
      }
    },
  );

  it("keeps dismissed reports dismissed after a fresh store and repeated seed", () => {
    const id = `${A}/51`;
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
    };
    const first = new WriteStore(storage);
    first.seed([reported(id)]);
    expect(first.pendingReport(A)?.id).toBe(id);
    first.dismissReport(id);
    const stored = [...values.entries()];
    expect(stored).toHaveLength(1);
    expect(stored[0]?.[1]).not.toContain("private markdown");
    expect(stored[0]?.[1]).not.toContain("at://x");

    const recovered = new WriteStore(storage);
    recovered.seed([reported(id), reported(`${A}/52`, "second report")]);
    expect(recovered.pendingReport(A)?.id).toBe(`${A}/52`);
    recovered.dismissReport(id);
    recovered.dismissReport(`${A}/52`);
    const again = new WriteStore(storage);
    again.seed([reported(id), reported(`${A}/52`, "second report")]);
    expect(again.pendingReport(A)).toBeUndefined();
  });

  it("keeps undismissed reports queued across recovery and another report opening", () => {
    const id = `${A}/61`;
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
    };
    const first = new WriteStore(storage);
    first.seed([reported(id), reported(`${A}/62`, "second report")]);
    const recovered = new WriteStore(storage);
    recovered.seed([reported(id), reported(`${A}/62`, "second report")]);
    expect(recovered.pendingReport(A)?.id).toBe(id);
    recovered.dismissReport(`${A}/62`);
    expect(recovered.pendingReport(A)?.id).toBe(id);
    const onceMore = new WriteStore(storage);
    onceMore.seed([reported(id), reported(`${A}/62`, "second report")]);
    expect(onceMore.pendingReport(A)?.id).toBe(id);
    onceMore.dismissReport(id);
    expect(onceMore.pendingReport(A)).toBeUndefined();
  });

  it("preserves the first report across event, ticket and seed orderings", () => {
    const id = `${A}/42`;
    const s = new WriteStore();
    s.seed([ticket(id, { kind: "pending" })]);
    s.track(ticket(id, { kind: "sending" }));
    s.update(id, { kind: "done", uri: "at://x", report: ["first report"] });
    s.track(reported(id, "later duplicate"));
    s.seed([reported(id, "later duplicate")]);
    expect(s.pendingReport(A)?.lines).toEqual(["first report"]);
  });

  it("shows a done adoption report recovered from the outbox, once (sec-8)", () => {
    const s = new WriteStore();
    const one = `${A}/1`;
    const two = `${A}/2`;
    const report = ["cannot adopt at://did:plc:example/record: not in effect"];
    s.track({ id: one, state: { kind: "done", uri: "at://x", report } });
    expect(s.pendingReport(A)).toEqual({ id: one, lines: report });
    // a write that completed before the app subscribed is not repeated from the listing
    s.seed([{ id: one, state: { kind: "done", uri: "at://x", report } }]);
    // one recovered on a later page load is shown too
    s.seed([{ id: two, state: { kind: "done", uri: "at://y", report } }]);
    expect(s.pendingReport(A)?.id).toBe(one);
    s.dismissReport(one);
    expect(s.pendingReport(A)?.id).toBe(two);
    s.dismissReport(two);
    expect(s.pendingReport(A)).toBeUndefined();
  });
});
