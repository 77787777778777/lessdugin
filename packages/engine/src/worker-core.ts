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
// `Target::UserPds` requests: GETs of the public records of the account's own repo
// (`com.atproto.repo.{listRecords, getRecord, describeRepo}`, `com.atproto.sync.{getLatestCommit,
// getRecord}` with `repo`/`did` = the account) are sent anonymously from here to the PDS bound
// at login (the engine's account row); every other request goes to the main thread's
// AuthBridge, which allows only the AUTH-4 writes and `getSession`.

import { type BlobStore, IdbBlobStore, MemoryBlobStore } from "./blobs.ts";
import { BridgeKv } from "./bridge-kv.ts";
import { checkUrl, type FetchDeps, fetchAnon } from "./fetch.ts";
import { createJsHost, type JsHostImports } from "./host.ts";
import { type IDBFactoryLike, IdbKv, type KvBackend, MemKv } from "./idb.ts";
import { ChangeRelay, type ChannelLike, LeaderElection } from "./leader.ts";
import { LocalLocks, type LockManagerLike } from "./locks.ts";
import {
  type AuthBridgePort,
  CHANGES_CHANNEL,
  type HttpResult,
  type PdsRequest,
  type WorkerInfo,
  type WorkerInit,
} from "./protocol.ts";
import type { AccountView, EngineError, EngineErrorCode, EngineEvent } from "./types.ts";

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
  // a trap (RuntimeError) or another JS exception
  return engineError("internal", e instanceof Error ? `${e.name}: ${e.message}` : String(e));
}

/** Public reads of an account's own repo: the method and its repo parameter. */
const PUBLIC_OWN_READS: Record<string, string> = {
  "com.atproto.repo.listRecords": "repo",
  "com.atproto.repo.getRecord": "repo",
  "com.atproto.repo.describeRepo": "repo",
  "com.atproto.sync.getLatestCommit": "did",
  "com.atproto.sync.getRecord": "did",
};

/** True for a `GET` public read of `req.account`'s own repo. */
export function isPublicOwnRead(req: PdsRequest): boolean {
  if (req.method !== "GET" || req.body !== undefined) return false;
  const m = /^\/xrpc\/([A-Za-z0-9.-]+)(?:\?(.*))?$/.exec(req.path);
  const param = m?.[1] ? PUBLIC_OWN_READS[m[1]] : undefined;
  if (!param) return false;
  return new URLSearchParams(m?.[2] ?? "").get(param) === req.account;
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
  private readonly pds = new Map<string, string>();
  private stores: { close?: () => void }[] = [];
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
      const c = await IdbKv.open(`${prefix}-cache`, "relaxed", idb);
      this.stores.push(c);
      cache = c;
      const b = await IdbBlobStore.open(`${prefix}-blobs`, idb.factory);
      this.stores.push(b);
      blobs = b;
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

  /** The PDS bound at login to `did` (from the engine's accounts). */
  private async pdsOf(did: string): Promise<string | undefined> {
    if (!this.pds.has(did)) {
      const accounts = JSON.parse(await this.callJson("accounts", "[]")) as AccountView[];
      this.pds.clear();
      for (const a of accounts) this.pds.set(a.did, a.pds);
    }
    return this.pds.get(did);
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
    const { cache, state, blobs } = await this.openStores(init);
    let locks = this.env.locks;
    if (!locks) {
      this.warn("Web Locks are unavailable: locks hold within this worker only", null);
      locks = new LocalLocks();
    }
    const userPds = async (req: PdsRequest): Promise<HttpResult> => {
      if (isPublicOwnRead(req)) {
        const pds = await this.pdsOf(req.account);
        if (pds) {
          const url = new URL(req.path, checkUrl(pds, fetchDeps.policy)).href;
          return fetchAnon(
            {
              method: "GET",
              url,
              headers: req.headers,
              maxBytes: req.maxBytes,
              timeoutMs: req.timeoutMs,
              redirects: 0,
            },
            fetchDeps,
          );
        }
      }
      return bridge.request(req);
    };
    const host = createJsHost({ cache, state, fetch: fetchDeps, userPds, blobs, locks });
    let engine: WasmEngineLike;
    try {
      engine = new this.wasm.WasmEngine(JSON.stringify(init.config), host);
    } catch (e) {
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
      if (event.kind === "changed" && event.topics.some((t) => t.kind === "accounts")) {
        this.pds.clear();
      }
      this.relay?.publish(event);
    });
    if (init.elect !== false) {
      this.leader = new LeaderElection({ locks, onLead: () => this.lead() });
      this.leader.start();
    }
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
      if (event.topics.some((t) => t.kind === "accounts")) this.pds.clear();
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
    await this.leader?.stop();
    this.relay?.close();
    if (this.subscription !== null) engine.unsubscribe(this.subscription);
    for (const s of this.stores) s.close?.();
    this.engine = null;
  }
}
