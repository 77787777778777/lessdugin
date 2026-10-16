// SPDX-License-Identifier: AGPL-3.0-or-later
// Main-thread client for the engine worker. Re-exports the DTO types (engine-api.md §5).

import * as Comlink from "comlink";
import type { EngineWorkerApi } from "./worker.ts";

export type { EngineApi } from "./api.ts";
export type * from "./types.ts";

export type EngineClient = Comlink.Remote<EngineWorkerApi>;

export function startEngine(): EngineClient {
  const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  return Comlink.wrap<EngineWorkerApi>(worker);
}
