// SPDX-License-Identifier: AGPL-3.0-or-later
// Main-thread client for the engine worker. Re-exports the DTO types (engine-api.md §5).
//
// `startEngine(options)` starts the worker (through the `lessdugin-worker` Trusted Types policy)
// and returns an `EngineClient`: the `EngineApi` of api.ts, each method forwarded to the worker
// (`WasmEngine.call`), plus `callJson` (the dto/1 text, for parity checks), `version()` and
// `terminate()`. The worker is initialised on first use (or through `ready`); `version()` only
// loads the module.
//
// The main thread keeps the session: `options.sessions` (hosted OAuth or the `lessdugin web`
// bridge, see auth-bridge.ts) backs the `AuthBridge` the worker sends its authenticated requests
// to. The worker reaches it, and sends its events, only through a dedicated `MessagePort` with a
// fixed message schema (bridge-port.ts): no main-thread object is exposed to the worker, which
// parses network data and is therefore untrusted (threat-model SPA-13, AUTH-4). Engine events
// reach `subscribe` listeners in order, never re-entrantly.
//
// `outboxPump()` and `liveTail()` resolve at once here: the worker runs them itself in the
// leader tab only (leader.ts). `grant`/`revoke` write one record per actor and resolve with
// the first actor's ticket (`approve`/`reject` resolve with all of them).

import * as Comlink from "comlink";
import type { EngineApi } from "./api.ts";
import { AuthBridge, NS, type SessionProvider } from "./auth-bridge.ts";
import { serveBridgePort } from "./bridge-port.ts";
import type { WorkerInfo, WorkerInit } from "./protocol.ts";
import type { EngineEvent, EventListener, Unsubscribe, WriteTicket } from "./types.ts";
import type { EngineWorkerApi } from "./worker.ts";
import { workerScriptUrl } from "./workerUrl.ts";

export type { EngineApi } from "./api.ts";
export type { PdsSession, SessionProvider } from "./auth-bridge.ts";
export {
  AuthBridge,
  bridgeSession,
  NS,
  oauthSession,
  RECORD_COLLECTIONS,
  UPLOAD_TYPES,
} from "./auth-bridge.ts";
export type {
  EngineConfigInput,
  HostErrorValue,
  StateStoreConfig,
  WorkerInfo,
  WorkerInit,
} from "./protocol.ts";
export { hostError } from "./protocol.ts";
export type * from "./types.ts";

export interface StartEngineOptions {
  /** The worker's start-up (configuration, stores, election); `namespace` is this build's. */
  init?: Omit<WorkerInit, "namespace">;
  /** The accounts' sessions (default: none, so writes fail with `auth_required`). */
  sessions?: SessionProvider;
  /** Creates the worker (default: the engine worker script). */
  worker?: () => Worker;
}

/** The engine as the SPA sees it. */
export interface EngineClient extends EngineApi {
  /** Resolves once the worker's engine runs (starting it if needed). */
  readonly ready: Promise<WorkerInfo>;
  /** The engine version (does not start the engine). */
  version(): Promise<string>;
  /** Runs `op` and resolves with its dto/1 JSON text, byte for byte what the CLI's `--json`
   * prints for the same DTO. */
  callJson(op: string, args: unknown[]): Promise<string>;
  /** Whether this tab's worker is the leader. */
  isLeader(): Promise<boolean>;
  /** Stops the worker (hands the leadership over first). */
  terminate(): Promise<void>;
}

const NO_SESSIONS: SessionProvider = { session: async () => null };

/** Operations whose trailing parameters are optional in `EngineApi`, with their full arity.
 * The wasm side reads the arguments as a fixed-size tuple (crates/wasm/src/ops.rs, `Option`
 * elements included), so an omitted trailing argument is sent as `null`. */
export const OP_ARITY: Readonly<Record<string, number>> = Object.freeze({
  parseRef: 2,
  forumFollow: 2,
  join: 2,
  members: 2,
  syncForum: 2,
  syncThread: 2,
  recentDiscussion: 3,
  getThread: 2,
  edit: 3,
  render: 3,
  verify: 2,
  modLog: 3,
  user: 2,
});

/** `args` with omitted trailing optional arguments as `null` (see `OP_ARITY`). */
export function padArgs(op: string, args: readonly unknown[]): unknown[] {
  const n = OP_ARITY[op];
  const out = [...args];
  while (n !== undefined && out.length < n) out.push(null);
  return out;
}

/** Methods with a client-side implementation. */
const LOCAL = new Set(["subscribe", "ready", "version", "callJson", "isLeader", "terminate"]);

export function startEngine(options: StartEngineOptions = {}): EngineClient {
  // The URL passes the `lessdugin-worker` Trusted Types policy (threat-model SPA-2).
  const worker = options.worker
    ? options.worker()
    : new Worker(workerScriptUrl(), { type: "module" });
  const remote = Comlink.wrap<EngineWorkerApi>(worker);
  const bridge = new AuthBridge(options.sessions ?? NO_SESSIONS);
  const listeners = new Set<EventListener>();
  const deliver = (json: string): void => {
    let event: EngineEvent;
    try {
      event = JSON.parse(json) as EngineEvent;
    } catch {
      return;
    }
    for (const l of [...listeners]) {
      try {
        l(event);
      } catch (e) {
        console.error("lessdugin engine: an event listener threw", e);
      }
    }
  };
  let started: Promise<WorkerInfo> | null = null;
  let stopServing: (() => void) | null = null;
  const start = (): Promise<WorkerInfo> => {
    if (!started) {
      // The worker's only way to the AuthBridge: one port, fixed messages (bridge-port.ts).
      const { port1, port2 } = new MessageChannel();
      stopServing = serveBridgePort(port1, {
        request: (req) => bridge.request(req),
        event: deliver,
      });
      const init: WorkerInit = { config: {}, ...options.init, namespace: NS };
      const p = remote.init(init, Comlink.transfer(port2, [port2]));
      started = p;
      // A failed start (IndexedDB blocked or over quota, …) is not cached: the next call tries
      // again on a fresh port.
      p.catch(() => {
        if (started !== p) return;
        started = null;
        stopServing?.();
        stopServing = null;
      });
    }
    return started;
  };
  const call = async (op: string, args: unknown[]): Promise<unknown> => {
    await start();
    return remote.call(op, padArgs(op, args));
  };

  const local = {
    get ready() {
      return start();
    },
    version: () => remote.version(),
    async callJson(op: string, args: unknown[]) {
      await start();
      return remote.callJson(op, JSON.stringify(padArgs(op, args)));
    },
    isLeader: () => remote.isLeader(),
    async terminate() {
      try {
        if (started) await remote.close();
      } finally {
        stopServing?.();
        remote[Comlink.releaseProxy]();
        worker.terminate();
      }
    },
    subscribe(listener: EventListener): Unsubscribe {
      listeners.add(listener);
      void start().catch(() => {});
      return () => {
        listeners.delete(listener);
      };
    },
  };

  const special: Record<string, (...args: unknown[]) => Promise<unknown>> = {
    outboxPump: async () => undefined,
    liveTail: async () => undefined,
    // The bytes cross the worker boundary as a structured-cloned array (a Uint8Array arrives as
    // an index-keyed object), so they are flattened to a plain number array here. A Uint8Array
    // that the structured clone left intact is spread just the same.
    async prepareMedia(...args) {
      const [bytes, input] = args as [Uint8Array | number[] | ArrayLike<number>, unknown];
      return remote.call("prepareMedia", [Array.from(bytes), input ?? null]);
    },
    async grant(...args) {
      return first(await call("grant", args));
    },
    async revoke(...args) {
      return first(await call("revoke", args));
    },
  };

  return new Proxy(local as unknown as EngineClient, {
    get(target, prop, receiver) {
      if (typeof prop !== "string" || prop === "then" || prop === "toJSON") return undefined;
      if (LOCAL.has(prop)) return Reflect.get(target, prop, receiver);
      const s = special[prop];
      if (s) return s;
      return (...args: unknown[]) => call(prop, args);
    },
  });
}

function first(tickets: unknown): WriteTicket {
  const list = tickets as WriteTicket[];
  const t = list[0];
  if (!t) throw { code: "invalid_input", message: "no actor given", retryable: false };
  return t;
}
