// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The engine worker's logic, apart from Comlink and the loading of the wasm module (worker.ts),
// so that tests can run it in node against a real wasm build.
//
// `WorkerEngine.init` opens the stores, builds the JS host (host.ts), creates the `WasmEngine`,
// forwards its events to the main thread and to the other tabs (leader.ts), and joins the leader
// election: the leader runs `outboxPump` and `liveTail`. Every engine operation then goes
// through `callJson(op, argsJson)` (dto/1 text, exactly as `dto.rs` serialises it) or
// `call(op, args)` (parsed). Failures reject with an `EngineError` object.
//
// `Target::UserPds` requests all go to the main thread's AuthBridge, which allows only the
// AUTH-4 writes and `getSession` (the core reads public records, an account's own included,
// anonymously: `Target::Anon`).

import { type BlobStore, IdbBlobStore, MemoryBlobStore } from "./blobs.ts";
import { BridgeKv } from "./bridge-kv.ts";
import type { FetchDeps } from "./fetch.ts";
import { createJsHost, type JsHostImports } from "./host.ts";
import { type IDBFactoryLike, IdbKv, type KvBackend, MemKv } from "./idb.ts";
import { ChangeRelay, type ChannelLike, LeaderElection } from "./leader.ts";
import { LocalLocks, type LockManagerLike } from "./locks.ts";
import {
  type AuthBridgePort,
  CHANGES_CHANNEL,
  type HttpResult,
  isHostError,
  LEADER_LOCK,
  type PdsRequest,
  type WorkerInfo,
  type WorkerInit,
} from "./protocol.ts";
import type { EngineError, EngineErrorCode, EngineEvent } from "./types.ts";

/** The wasm-bindgen exports the worker uses (`pkg/lessdugin_engine.js`). */
export interface WasmModule {
  WasmEngine: new (configJson: string, host: JsHostImports) => WasmEngineLike;
  engineVersion(): string;
  engineNamespace(): string;
}

export interface WasmEngineLike {
  call(op: string, argsJson: string): Promise<string>;
  subscribe(cb: (eventJson: string) => void): number;
  unsubscribe(id: number): boolean;
  free?(): void;
}

/** The platform of the worker (globals in a browser; injected in tests). */
export interface WorkerEnv {
  fetch: typeof fetch;
  /** `navigator.locks`; `null`: in-realm locks only (a warning is logged). */
  locks: LockManagerLike | null;
  /** IndexedDB; `null`: in-memory stores. */
  idb: IDBFactoryLike | null;
  onLine?: () => boolean;
  /** Opens the change channel (`null`: no other tabs). */
  channel?: ((name: string) => ChannelLike) | null;
  /** Logs a failure of a background task. */
  warn?: (message: string, error: unknown) => void;
  /** Calls `cb` when the device comes back online (`online` on the worker's global); returns
   * the unsubscribe function. */
  onOnline?: (cb: () => void) => () => void;
}

export function engineError(code: EngineErrorCode, message: string): EngineError {
  return { code, message, retryable: false };
}

/** A wasm rejection (an `EngineError` JSON string) as an `EngineError` object. */
export function toEngineError(e: unknown): EngineError {
  if (typeof e === "string") {
    try {
      const v = JSON.parse(e) as EngineError;
      if (typeof v.code === "string" && typeof v.message === "string") return v;
    } catch {
      // not JSON
    }
    return engineError("internal", e);
  }
  if (typeof e === "object" && e !== null && "code" in e && "message" in e) return e as EngineError;
  if (isHostError(e)) {
    // a host failure outside the engine (opening a store): storage problems may pass
    const code: EngineErrorCode = e.kind === "blocked" ? "blocked" : "internal";
    return { code, message: `${e.kind}: ${e.message}`, retryable: e.kind === "storage" };
  }
  // a trap (RuntimeError) or another JS exception
  return engineError("internal", e instanceof Error ? `${e.name}: ${e.message}` : String(e));
}

function randomId(): string {
  const b = new Uint8Array(8);
  crypto.getRandomValues(b);
  return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}

export class WorkerEngine {
  private engine: WasmEngineLike | null = null;
  private subscription: number | null = null;
  private leader: LeaderElection | null = null;
  private relay: ChangeRelay | null = null;
  private stores: { close?: () => void }[] = [];
  private offOnline: (() => void) | null = null;
  readonly id = randomId();

  constructor(
    private readonly wasm: WasmModule,
    private readonly env: WorkerEnv,
  ) {}

  get isLeader(): boolean {
    return this.leader?.isLeader ?? false;
  }

  version(): string {
    return this.wasm.engineVersion();
  }

  private warn(message: string, e: unknown): void {
    (this.env.warn ?? ((m, err) => console.warn(`lessdugin engine: ${m}`, err)))(message, e);
  }

  private async openStores(
    init: WorkerInit,
  ): Promise<{ cache: KvBackend; state: KvBackend; blobs: BlobStore }> {
    const prefix = init.dbPrefix ?? "lessdugin";
    const idb = this.env.idb;
    const stateCfg = init.state ?? { kind: "indexeddb" };
    let cache: KvBackend;
    let blobs: BlobStore;
    if (idb) {
      // The cache and the blobs can always be fetched again: if IndexedDB refuses them (a
      // private window, a full disk), this tab keeps them in memory. `state` never falls back.
      try {
        const c = await IdbKv.open(`${prefix}-cache`, "relaxed", idb);
        this.stores.push(c);
        cache = c;
      } catch (e) {
        this.warn("the cache store could not be opened: it is kept in memory", e);
        cache = new MemKv();
      }
      try {
        const b = await IdbBlobStore.open(`${prefix}-blobs`, idb.factory);
        this.stores.push(b);
        blobs = b;
      } catch (e) {
        this.warn("the blob store could not be opened: blobs are kept in memory", e);
        blobs = new MemoryBlobStore();
      }
    } else {
      cache = new MemKv();
      blobs = new MemoryBlobStore();
    }
    let state: KvBackend;
    if (stateCfg.kind === "bridge") {
      state = new BridgeKv({
        endpoint: stateCfg.endpoint,
        token: stateCfg.token,
        fetch: this.env.fetch,
      });
    } else if (stateCfg.kind === "memory" || !idb) {
      state = new MemKv();
    } else {
      const s = await IdbKv.open(`${prefix}-state`, "strict", idb);
      this.stores.push(s);
      state = s;
    }
    return { cache, state, blobs };
  }

  async init(
    init: WorkerInit,
    bridge: AuthBridgePort,
    onEvent: (eventJson: string) => void,
  ): Promise<WorkerInfo> {
    if (this.engine) throw engineError("conflict", "the engine worker is already initialised");
    const ns = this.wasm.engineNamespace();
    if (ns !== init.namespace) {
      throw engineError(
        "internal",
        `the engine writes ${ns} but the AuthBridge allows ${init.namespace}: mismatched builds`,
      );
    }
    const devnet = init.config.network?.kind === "devnet";
    const fetchDeps: FetchDeps = {
      fetch: this.env.fetch,
      policy: { devnet },
      ...(this.env.onLine ? { onLine: this.env.onLine } : {}),
    };
    let stores: Awaited<ReturnType<WorkerEngine["openStores"]>>;
    try {
      stores = await this.openStores(init);
    } catch (e) {
      this.closeStores();
      throw toEngineError(e);
    }
    const { cache, state, blobs } = stores;
    let locks = this.env.locks;
    if (!locks) {
      this.warn("Web Locks are unavailable: locks hold within this worker only", null);
      locks = new LocalLocks();
    }
    const userPds = (req: PdsRequest): Promise<HttpResult> => bridge.request(req);
    // The host lock `sync-leader` is the leader's Web Lock (sync.md §10.6). Web Locks are not
    // re-entrant, so the leader's own engine would wait for itself: it already holds it.
    const shared = locks;
    const hostLocks: LockManagerLike = {
      request: (name, options, callback) =>
        name === LEADER_LOCK && this.isLeader
          ? Promise.resolve().then(() => callback({ name, mode: "exclusive" }))
          : shared.request(name, options, callback),
    };
    const host = createJsHost({
      cache,
      state,
      fetch: fetchDeps,
      userPds,
      blobs,
      locks: hostLocks,
      bridgeLock:
        init.state?.kind === "bridge"
          ? {
              endpoint: init.state.lockEndpoint ?? "/bridge/lock",
              token: init.state.token,
              fetch: this.env.fetch,
              onLost: (name) =>
                this.warn(`the native ${name} lock ended before it was released`, null),
            }
          : undefined,
    });
    let engine: WasmEngineLike;
    try {
      engine = new this.wasm.WasmEngine(JSON.stringify(init.config), host);
    } catch (e) {
      this.closeStores();
      throw toEngineError(e);
    }
    this.engine = engine;

    const channel = this.env.channel?.(CHANGES_CHANNEL) ?? null;
    if (channel) {
      this.relay = new ChangeRelay({
        channel,
        id: this.id,
        onEvent: (event) => void this.remoteEvent(event, onEvent),
      });
    }
    this.subscription = engine.subscribe((json) => {
      onEvent(json);
      let event: EngineEvent;
      try {
        event = JSON.parse(json) as EngineEvent;
      } catch {
        return;
      }
      this.relay?.publish(event);
    });
    if (init.elect !== false) {
      this.leader = new LeaderElection({ locks, onLead: () => this.lead() });
      this.leader.start();
    }
    // Back online: the leader's pump looks at its queue again at once (robustness review H3).
    this.offOnline =
      this.env.onOnline?.(() => {
        if (!this.isLeader) return;
        this.callJson("outboxWake", "[]").catch((e: unknown) => this.warn("outboxWake failed", e));
      }) ?? null;
    return { version: this.wasm.engineVersion(), namespace: ns, id: this.id };
  }

  /** This worker became the leader: run the long-lived tasks. */
  private lead(): void {
    for (const op of ["outboxPump", "liveTail"]) {
      this.callJson(op, "[]").catch((e: unknown) => this.warn(`${op} stopped`, e));
    }
  }

  private async remoteEvent(event: EngineEvent, onEvent: (json: string) => void): Promise<void> {
    if (event.kind === "changed") {
      try {
        await this.callJson("externalChange", JSON.stringify([event.topics]));
      } catch (e) {
        this.warn("externalChange failed", e);
      }
    } else if (event.kind === "write" && event.state.kind === "pending" && this.isLeader) {
      // another tab queued (or re-queued) a write: the pump picks it up now
      this.callJson("outboxWake", "[]").catch((e: unknown) => this.warn("outboxWake failed", e));
    }
    onEvent(JSON.stringify(event));
  }

  /** Runs `op` with `argsJson` (a JSON array); resolves with the dto/1 JSON text. */
  async callJson(op: string, argsJson: string): Promise<string> {
    const engine = this.engine;
    if (!engine) throw engineError("internal", "the engine worker is not initialised");
    try {
      return await engine.call(op, argsJson);
    } catch (e) {
      throw toEngineError(e);
    }
  }

  /** Runs `op` with `args` (`undefined` becomes `null`); resolves with the parsed result. */
  async call(op: string, args: unknown[]): Promise<unknown> {
    return JSON.parse(await this.callJson(op, JSON.stringify(args))) as unknown;
  }

  /** Stops the long-lived tasks, hands the leadership over and closes the stores. */
  async close(): Promise<void> {
    const engine = this.engine;
    if (!engine) return;
    await Promise.allSettled([this.callJson("outboxStop", "[]"), this.callJson("liveStop", "[]")]);
    this.offOnline?.();
    this.offOnline = null;
    await this.leader?.stop();
    this.relay?.close();
    if (this.subscription !== null) engine.unsubscribe(this.subscription);
    this.closeStores();
    this.engine = null;
  }

  private closeStores(): void {
    for (const s of this.stores.splice(0)) s.close?.();
  }
}
