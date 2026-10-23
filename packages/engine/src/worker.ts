// SPDX-License-Identifier: AGPL-3.0-or-later
// The engine worker: loads the Rust engine (wasm), builds its JS host (worker-core.ts) and
// exposes it over Comlink (`EngineWorkerApi`). The main thread talks to it through client.ts.
// The way back (AuthBridge requests and engine events) is the `MessagePort` handed over in
// `init`, with the fixed messages of bridge-port.ts; the main thread exposes no object here.
//
// Test-only configuration is gated by build-time flags, which Vite replaces with literals so
// release builds fold the checks and never pass the values to the engine:
// - `network: devnet` (http, loopback; threat-model INFRA-9) needs a development build or an
//   e2e build (`VITE_E2E=1`);
// - `parityNowMs` (engine-api.md §2.1) needs an e2e build.

import * as Comlink from "comlink";
import initWasm, * as wasm from "../pkg/lessdugin_engine.js";
import { connectBridgePort } from "./bridge-port.ts";
import { globalIdb } from "./idb.ts";
import type { LockManagerLike } from "./locks.ts";
import type { WorkerInfo, WorkerInit } from "./protocol.ts";
import { engineError, WorkerEngine } from "./worker-core.ts";

const E2E_BUILD: boolean = import.meta.env.VITE_E2E === "1";
const DEVNET_BUILD: boolean = import.meta.env.DEV === true || E2E_BUILD;

// A CSP or Trusted Types violation inside this worker fires `securitypolicyviolation` on the
// worker's global only: the page's listeners never see it. Every one is reported on the console
// (there is no reporting endpoint, threat-model §6.1), which the e2e suites fail on; the blocked
// URL is reduced to its origin.
function blockedWhat(uri: string): string {
  try {
    return new URL(uri).origin;
  } catch {
    return uri.slice(0, 40); // "eval", "inline", "trusted-types-sink", …
  }
}
globalThis.addEventListener("securitypolicyviolation", (e) => {
  const v = e as SecurityPolicyViolationEvent;
  console.error(
    `lessdugin engine worker: csp-violation ${v.effectiveDirective || v.violatedDirective} blocked=${blockedWhat(v.blockedURI)}`,
  );
});

// streaming compile of the module next to the glue (`new URL(…, import.meta.url)`)
const loaded = initWasm();
let engine: WorkerEngine | null = null;
let starting = false;

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

  /** Starts the engine: stores, host, events (sent in order on `port`), election. `port` is
   * the main thread's AuthBridge link (bridge-port.ts). */
  async init(init: WorkerInit, port: MessagePort): Promise<WorkerInfo> {
    await loaded;
    if (!E2E_BUILD && init.config.parityNowMs !== undefined) {
      throw engineError("blocked", "parity mode is test-only: this build does not accept it");
    }
    if (!DEVNET_BUILD && init.config.network?.kind === "devnet") {
      throw engineError("blocked", "devnet mode needs a development or e2e build");
    }
    if (engine || starting) throw engineError("conflict", "the engine worker is already started");
    const nav = globalThis.navigator as Navigator & { locks?: LockManagerLike };
    const e = new WorkerEngine(wasm, {
      fetch: globalThis.fetch.bind(globalThis),
      locks: nav.locks ?? null,
      idb: globalIdb(),
      onLine: () => nav.onLine !== false,
      channel: typeof BroadcastChannel === "undefined" ? null : (n) => new BroadcastChannel(n),
      onOnline: (cb) => {
        globalThis.addEventListener("online", cb);
        return () => globalThis.removeEventListener("online", cb);
      },
    });
    const link = connectBridgePort(port);
    starting = true;
    try {
      const info = await e.init(init, link.bridge, (json) => link.emit(json));
      engine = e;
      if (E2E_BUILD && init.cspProbe === true) {
        try {
          // biome-ignore lint/security/noGlobalEval: e2e-only probe of the worker's CSP watcher
          globalThis.eval("1");
        } catch {
          // blocked by `script-src` (the point)
        }
      }
      return info;
    } catch (err) {
      // a failed start leaves nothing behind, so the main thread can try again
      link.close();
      throw err;
    } finally {
      starting = false;
    }
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
