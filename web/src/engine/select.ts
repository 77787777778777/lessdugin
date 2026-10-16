// SPDX-License-Identifier: AGPL-3.0-or-later
// Which engine the app runs against. The in-memory mock (fixture data, a mock sign-in) exists
// only in development and in demo builds (`VITE_DEMO=1`, or `VITE_ENGINE=mock`); there `?mock`
// in the URL (before or inside the hash) selects it. A release build ignores `?mock` entirely
// and does not even contain the mock: the dynamic import below is dead code there, so a link to
// the real origin cannot open a fixture world with a working fake sign-in.

import type { EngineHandle } from "./handle.ts";

export type EngineChoice = "mock" | "wasm";

export interface EngineEnv {
  VITE_ENGINE?: string | undefined;
  VITE_DEMO?: string | undefined;
  DEV?: boolean;
}

/** Whether this build may run the mock engine. */
export function mockAllowed(env: EngineEnv): boolean {
  return env.DEV === true || env.VITE_DEMO === "1" || env.VITE_ENGINE === "mock";
}

export function chooseEngine(loc: { search: string; hash: string }, env: EngineEnv): EngineChoice {
  if (!mockAllowed(env)) return "wasm";
  if (new URLSearchParams(loc.search).has("mock")) return "mock";
  const hashQuery = loc.hash.split("?")[1];
  if (hashQuery !== undefined && new URLSearchParams(hashQuery).has("mock")) return "mock";
  if (env.VITE_ENGINE === "mock") return "mock";
  if (env.VITE_ENGINE === "wasm") return "wasm";
  return env.DEV ? "mock" : "wasm";
}

/** Statically known per build (Vite replaces `import.meta.env.*`), so release builds drop the
 * mock chunk. */
const MOCK_BUILD =
  import.meta.env.DEV ||
  import.meta.env.VITE_DEMO === "1" ||
  import.meta.env.VITE_ENGINE === "mock";

export async function loadEngine(choice: EngineChoice): Promise<EngineHandle> {
  if (choice === "mock") {
    if (!MOCK_BUILD) throw new Error("this build does not include the demo engine");
    const { createMockHandle } = await import("./mock.ts");
    return createMockHandle();
  }
  const { createRealEngine } = await import("./real.ts");
  return createRealEngine();
}
