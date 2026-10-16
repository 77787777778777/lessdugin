// SPDX-License-Identifier: AGPL-3.0-or-later
// Main-thread client for the engine worker. Re-exports the DTO types (engine-api.md §5).

import * as Comlink from "comlink";
import type { EngineWorkerApi } from "./worker.ts";
import { workerScriptUrl } from "./workerUrl.ts";

export type { EngineApi } from "./api.ts";
export type * from "./types.ts";

export type EngineClient = Comlink.Remote<EngineWorkerApi>;

export function startEngine(): EngineClient {
  // The URL passes the `lessdugin-worker` Trusted Types policy (threat-model SPA-2).
  const worker = new Worker(workerScriptUrl(), { type: "module" });
  return Comlink.wrap<EngineWorkerApi>(worker);
}
