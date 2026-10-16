// SPDX-License-Identifier: AGPL-3.0-or-later
// Which engine the app runs against: the mock when the URL has `?mock` (before or inside the
// hash), or when VITE_ENGINE=mock, or by default in development; otherwise the wasm adapter.

import type { EngineHandle } from "./handle.ts";

export type EngineChoice = "mock" | "wasm";

export function chooseEngine(
  loc: { search: string; hash: string },
  env: { VITE_ENGINE?: string | undefined; DEV?: boolean },
): EngineChoice {
  if (new URLSearchParams(loc.search).has("mock")) return "mock";
  const hashQuery = loc.hash.split("?")[1];
  if (hashQuery !== undefined && new URLSearchParams(hashQuery).has("mock")) return "mock";
  if (env.VITE_ENGINE === "mock") return "mock";
  if (env.VITE_ENGINE === "wasm") return "wasm";
  return env.DEV ? "mock" : "wasm";
}

export async function loadEngine(choice: EngineChoice): Promise<EngineHandle> {
  if (choice === "mock") {
    const { createMockHandle } = await import("./mock.ts");
    return createMockHandle();
  }
  const { createRealEngine } = await import("./real.ts");
  return createRealEngine();
}
