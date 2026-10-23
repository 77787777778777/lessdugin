// SPDX-License-Identifier: AGPL-3.0-or-later
// The engine worker: loads the Rust engine (wasm), builds its JS host (worker-core.ts) and
// exposes it over Comlink (`EngineWorkerApi`). The main thread talks to it through client.ts.
//
// Test-only configuration is gated by build-time flags, which Vite replaces with literals so
// release builds fold the checks and never pass the values to the engine:
// - `network: devnet` (http, loopback; threat-model INFRA-9) needs a development build or an
//   e2e build (`VITE_E2E=1`);
// - `parityNowMs` (engine-api.md §2.1) needs an e2e build.

import * as Comlink from "comlink";
import initWasm, * as wasm from "../pkg/lessdugin_engine.js";
import { globalIdb } from "./idb.ts";
import type { LockManagerLike } from "./locks.ts";
import type { AuthBridgePort, WorkerInfo, WorkerInit } from "./protocol.ts";
import { engineError, WorkerEngine } from "./worker-core.ts";

const E2E_BUILD: boolean = import.meta.env.VITE_E2E === "1";
const DEVNET_BUILD: boolean = import.meta.env.DEV === true || E2E_BUILD;

// streaming compile of the module next to the glue (`new URL(…, import.meta.url)`)
const loaded = initWasm();
let engine: WorkerEngine | null = null;

function current(): WorkerEngine {
  if (!engine) throw engineError("internal", "the engine worker is not initialised");
  return engine;
}

const api = {
  /** The engine version (loads the module, does not start the engine). */
  async version(): Promise<string> {
    await loaded;
    return wasm.engineVersion();
  },

  /** Starts the engine: stores, host, events (`onEvent(eventJson)`, in order), election. */
  async init(
    init: WorkerInit,
    bridge: AuthBridgePort,
    onEvent: (eventJson: string) => void,
  ): Promise<WorkerInfo> {
    await loaded;
    if (!E2E_BUILD && init.config.parityNowMs !== undefined) {
      throw engineError("blocked", "parity mode is test-only: this build does not accept it");
    }
    if (!DEVNET_BUILD && init.config.network?.kind === "devnet") {
      throw engineError("blocked", "devnet mode needs a development or e2e build");
    }
    const nav = globalThis.navigator as Navigator & { locks?: LockManagerLike };
    engine = new WorkerEngine(wasm, {
      fetch: globalThis.fetch.bind(globalThis),
      locks: nav.locks ?? null,
      idb: globalIdb(),
      onLine: () => nav.onLine !== false,
      channel: typeof BroadcastChannel === "undefined" ? null : (n) => new BroadcastChannel(n),
    });
    return engine.init(init, bridge, onEvent);
  },

  /** Runs an operation; resolves with the parsed dto/1 result. */
  call(op: string, args: unknown[]): Promise<unknown> {
    return current().call(op, args);
  },

  /** Runs an operation; resolves with the dto/1 JSON text exactly as `dto.rs` serialises it
   * (parity checks compare these bytes with the CLI's `--json`). */
  callJson(op: string, argsJson: string): Promise<string> {
    return current().callJson(op, argsJson);
  },

  /** Whether this worker is the leader tab's. */
  isLeader(): boolean {
    return engine?.isLeader ?? false;
  },

  /** Stops the engine (hands the leadership over). */
  async close(): Promise<void> {
    await engine?.close();
    engine = null;
  },
};

export type EngineWorkerApi = typeof api;

Comlink.expose(api);
