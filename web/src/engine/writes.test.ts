// SPDX-License-Identifier: AGPL-3.0-or-later
// The write store: seeding from `writes()`, and `settle`, which lets a composer go once a write
// waits in the outbox instead of spinning until it is sent (robustness review H3).
import { describe, expect, it } from "vitest";
import { canSendNow, QUEUED_AFTER_MS, WriteStore, waitingText } from "./writes.ts";

const NOW = 1_000_000;

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
});
