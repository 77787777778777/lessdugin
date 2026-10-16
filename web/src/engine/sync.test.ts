// SPDX-License-Identifier: AGPL-3.0-or-later
import type { EngineApi } from "@lessdugin/engine/api";
import { describe, expect, it, vi } from "vitest";
import { startEngineTasks, syncProgress } from "./sync.ts";

describe("host-driven sync (engine-api.md §3.3)", () => {
  it("starts the engine's long-lived tasks once per engine", () => {
    const never = () => new Promise<void>(() => {});
    const engine = {
      liveTail: vi.fn(never),
      outboxPump: vi.fn(never),
      backgroundVerify: vi.fn(never),
    } as unknown as EngineApi;
    startEngineTasks(engine);
    startEngineTasks(engine);
    expect(engine.liveTail).toHaveBeenCalledTimes(1);
    expect(engine.outboxPump).toHaveBeenCalledTimes(1);
    expect(engine.backgroundVerify).toHaveBeenCalledTimes(1);
  });

  it("tracks a forum's sync progress until it completes", () => {
    syncProgress.update("f", 2, 5);
    expect(syncProgress.get("f")).toEqual({ done: 2, total: 5 });
    syncProgress.update("f", 5, 5);
    expect(syncProgress.get("f")).toBeNull();
  });
});
