// SPDX-License-Identifier: AGPL-3.0-or-later
// The outbox page lists the writes queued before the page loaded (from `writes()`), says why
// each one waits, and retries a failed one (robustness review H3).
import { MockEngine } from "@lessdugin/engine/mock";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { OutboxPage, stateText } from "./OutboxPage.tsx";

describe("OutboxPage", () => {
  it("shows earlier writes and retries a failed one", async () => {
    const engine = new MockEngine();
    vi.spyOn(engine, "writes").mockResolvedValue([
      {
        id: "wfail",
        state: {
          kind: "failed",
          error: { code: "network", message: "the PDS is down", retryable: true },
        },
      },
      {
        id: "wwait",
        state: { kind: "pending", notBeforeMs: Date.now() + 60_000, throttled: true },
      },
      { id: "wdone", state: { kind: "done", uri: "at://x" } },
    ]);
    const retry = vi
      .spyOn(engine, "retryWrite")
      .mockResolvedValue({ id: "wfail", state: { kind: "pending" } });
    renderWithApp(<OutboxPage />, { engine });
    const failed = await screen.findByText(/Not sent: the PDS is down/);
    expect(failed.closest("[data-write-id]")?.getAttribute("data-write-id")).toBe("wfail");
    expect(screen.getByText(/Waiting: it will be sent when the connection allows/)).toBeTruthy();
    expect(document.querySelector('[data-write-id="wdone"]')).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(retry).toHaveBeenCalledWith("wfail"));
    await screen.findByText("Queued: it is sent next.");
  });

  it("sends a backing-off write now and points a write that needs a login to sign-in", async () => {
    const engine = new MockEngine();
    const error = { code: "network" as const, message: "connection reset", retryable: true };
    vi.spyOn(engine, "writes").mockResolvedValue([
      {
        id: "wback",
        state: { kind: "pending", notBeforeMs: Date.now() + 60_000, throttled: true, error },
      },
      {
        id: "wlimit",
        state: {
          kind: "pending",
          notBeforeMs: Date.now() + 60_000,
          throttled: true,
          error: { code: "rate_limited", message: "slow down", retryable: true },
        },
      },
      {
        id: "wlogin",
        state: {
          kind: "pending",
          needsLogin: true,
          error: { code: "auth_required", message: "expired", retryable: true },
        },
      },
    ]);
    const retry = vi
      .spyOn(engine, "retryWrite")
      .mockResolvedValue({ id: "wback", state: { kind: "pending" } });
    renderWithApp(<OutboxPage />, { engine });
    await screen.findByText(/last try: connection reset/);
    // only the write that backs off after a failure can be sent now (engine-api.md §3.5)
    const send = screen.getAllByRole("button", { name: "Send now" });
    expect(send).toHaveLength(1);
    expect(send[0]?.closest("[data-write-id]")?.getAttribute("data-write-id")).toBe("wback");
    const login = screen.getByText(/the session ended; it is sent after you sign in again/);
    const card = login.closest("[data-write-id]");
    expect(card?.getAttribute("data-write-id")).toBe("wlogin");
    expect(card?.querySelector("a")?.textContent).toBe("Sign in again");
    fireEvent.click(send[0] as HTMLElement);
    await waitFor(() => expect(retry).toHaveBeenCalledWith("wback"));
  });

  it("describes each state", () => {
    expect(stateText({ kind: "pending", held: true })).toMatch(/undo/);
    expect(stateText({ kind: "sending" })).toBe("Sending…");
    expect(stateText({ kind: "pending" })).toBe("Queued: it is sent next.");
  });
});
