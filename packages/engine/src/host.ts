// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The JS host object of the engine worker: what `new WasmEngine(configJson, host)` calls for the
// core's `Host` trait (crates/wasm/src/host.rs `JsHostImports`, sync.md §12). The pieces:
//
// - `http(...)`: `Target::Anon` requests through `fetchAnon` (fetch.ts); `Target::UserPds`
//   requests through `userPds` (the worker: the main thread's AuthBridge);
// - `fetchBlob(...)`: blobs.ts, one download per CID at a time (sync.md §12.2): a second call
//   for a CID being downloaded awaits the same promise, and a caller that stops waiting (the
//   core's `BODY_BLOB_WAIT_MS` for a spilled body) neither stops it nor starts another;
//   `blobOpen`, `blobRead`, `blobDelete`: the same store, by CID (`blobRead` slices the stored
//   `Blob`, so only the window it returns enters wasm memory);
// - `websocket(url, maxBytes)`: a native browser WebSocket, with one bounded-message FIFO per
//   socket (sync.md §6.2, §12.1); the handle pulls messages and sends v1 options updates;
// - `lock(name, wait)`: Web Locks (locks.ts); in `lessdugin web` mode an `outbox:<did>` lock is
//   then also the native one (bridge-lock.ts), Web Lock first;
// - `sleep(ms)`, `now()`, `random(n)`: `setTimeout`, `Date.now()`, `crypto.getRandomValues`;
// - `cache`, `state`: `KvBackend`s (idb.ts, bridge-kv.ts).

import { type BlobHandleValue, type BlobStore, fetchBlob } from "./blobs.ts";
import { type BridgeLockOptions, isBridgedLock, takeBridgeLock } from "./bridge-lock.ts";
import { type FetchDeps, fetchAnon } from "./fetch.ts";
import type { KvBackend } from "./idb.ts";
import { acquireLock, type LockHandle, type LockManagerLike } from "./locks.ts";
import {
  type AnonRequest,
  type HttpResult,
  hostError,
  isHostError,
  type PdsRequest,
} from "./protocol.ts";

/** A frame in the shape read by the wasm host. Exactly one of `text` and `binary` is present. */
export type JsWsMessage =
  | { kind: "message"; text: string }
  | { kind: "message"; binary: Uint8Array };

/** One open WebSocket. Calls to `next` drain only this socket's FIFO. */
export interface JsWebSocketHandle {
  next(): Promise<JsWsMessage | null>;
  sendText(text: string): Promise<void>;
  close(): Promise<void>;
}

/** The object the wasm `JsHost` calls. */
export interface JsHostImports {
  http(
    method: string,
    url: string | undefined,
    account: string | undefined,
    path: string | undefined,
    headers: string[],
    body: Uint8Array | undefined,
    maxBytes: number,
    timeoutMs: number,
    redirects: number,
  ): Promise<HttpResult>;
  fetchBlob(pds: string, did: string, cid: string, maxBytes: number): Promise<BlobHandleValue>;
  blobOpen(cid: string): Promise<BlobHandleValue | null>;
  blobRead(cid: string, offset: number, len: number): Promise<Uint8Array>;
  blobDelete(cid: string): Promise<void>;
  websocket(url: string, maxBytes: number): Promise<JsWebSocketHandle>;
  lock(name: string, wait: boolean): Promise<LockHandle | null>;
  sleep(ms: number): Promise<void>;
  now(): number;
  random(n: number): Uint8Array;
  readonly cache: KvBackend;
  readonly state: KvBackend;
}

export interface HostDeps {
  cache: KvBackend;
  state: KvBackend;
  fetch: FetchDeps;
  /** `Target::UserPds` requests. */
  userPds(req: PdsRequest): Promise<HttpResult>;
  blobs: BlobStore;
  locks: LockManagerLike;
  /** Native browser WebSocket (injectable for the worker's scripted-socket tests). */
  webSocket?: typeof WebSocket | undefined;
  /** `lessdugin web` mode: `outbox:<did>` locks are also taken natively (`POST /bridge/lock`). */
  bridgeLock?: BridgeLockOptions | undefined;
  /** Default `Date.now`. */
  now?: () => number;
}

/** sync.md §12.2: the most bytes one `blobRead` returns (`BLOB_READ_MAX_BYTES`, 1 MiB). */
export const BLOB_READ_MAX_BYTES = 1_048_576;

function pairs(flat: string[]): [string, string][] {
  const out: [string, string][] = [];
  for (let i = 0; i + 1 < flat.length; i += 2) out.push([flat[i] as string, flat[i + 1] as string]);
  return out;
}

/** Rejections the wasm side understands: host errors pass through, anything else is `other`. */
function asHostError(e: unknown): unknown {
  if (isHostError(e)) return e;
  return hostError("other", e instanceof Error ? `${e.name}: ${e.message}` : String(e));
}

type WsQueueItem =
  | { kind: "message"; message: JsWsMessage }
  | { kind: "error"; error: unknown }
  | { kind: "close" };

interface WsWaiter {
  resolve(message: JsWsMessage | null): void;
  reject(error: unknown): void;
}

/** The event queue and terminal state of exactly one browser WebSocket. */
class BrowserWebSocket implements JsWebSocketHandle {
  private readonly queue: WsQueueItem[] = [];
  private readonly waiters: WsWaiter[] = [];
  private ended = false;

  constructor(
    private readonly socket: WebSocket,
    private readonly maxBytes: number,
  ) {
    socket.binaryType = "arraybuffer";
    socket.addEventListener("message", this.onMessage);
    socket.addEventListener("error", this.onError);
    socket.addEventListener("close", this.onClose);
  }

  next(): Promise<JsWsMessage | null> {
    return new Promise((resolve, reject) => {
      this.waiters.push({ resolve, reject });
      this.drain();
    });
  }

  async sendText(text: string): Promise<void> {
    if (this.ended || this.socket.readyState !== 1) {
      throw hostError("network", "the WebSocket is closed");
    }
    try {
      this.socket.send(text);
    } catch (e) {
      throw asHostError(e);
    }
  }

  close(): Promise<void> {
    this.finish();
    return Promise.resolve();
  }

  private readonly onMessage = (event: MessageEvent): void => {
    if (this.ended) return;
    let message: JsWsMessage;
    let size: number;
    if (typeof event.data === "string") {
      size = new TextEncoder().encode(event.data).byteLength;
      message = { kind: "message", text: event.data };
    } else if (event.data instanceof ArrayBuffer) {
      size = event.data.byteLength;
      message = { kind: "message", binary: new Uint8Array(event.data) };
    } else {
      this.finish(hostError("network", "the WebSocket returned a non-ArrayBuffer binary frame"));
      return;
    }
    if (size > this.maxBytes) {
      this.finish(hostError("tooLarge", "WebSocket message exceeds the configured byte limit"));
      return;
    }
    this.queue.push({ kind: "message", message });
    this.drain();
  };

  private readonly onError = (): void => {
    this.finish(hostError("network", "WebSocket connection failed"));
  };

  private readonly onClose = (): void => {
    this.finish();
  };

  /** Queued frames remain readable; an error is followed by end-of-stream on the next pull. */
  private finish(error?: unknown): void {
    if (this.ended) return;
    this.ended = true;
    if (error !== undefined) this.queue.push({ kind: "error", error });
    this.queue.push({ kind: "close" });
    this.drain();
    this.socket.removeEventListener("message", this.onMessage);
    this.socket.removeEventListener("error", this.onError);
    this.socket.removeEventListener("close", this.onClose);
    try {
      this.socket.close();
    } catch {
      // Closing an already-closed socket is intentionally harmless.
    }
  }

  private drain(): void {
    while (this.queue.length > 0 && this.waiters.length > 0) {
      const item = this.queue.shift() as WsQueueItem;
      const waiter = this.waiters.shift() as WsWaiter;
      if (item.kind === "message") waiter.resolve(item.message);
      else if (item.kind === "error") waiter.reject(asHostError(item.error));
      else waiter.resolve(null);
    }
    if (this.ended && this.queue.length === 0) {
      for (const waiter of this.waiters.splice(0)) waiter.resolve(null);
    }
  }
}

async function openWebSocket(
  WebSocketImpl: typeof WebSocket | undefined,
  url: string,
  maxBytes: number,
): Promise<JsWebSocketHandle> {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "wss:" && parsed.protocol !== "ws:") {
      throw hostError("blocked", `not a WebSocket URL: ${parsed.protocol}`);
    }
    if (!Number.isSafeInteger(maxBytes) || maxBytes < 0) {
      throw hostError("other", "invalid WebSocket maximum message size");
    }
    if (!WebSocketImpl) throw hostError("unsupported", "WebSocket is unavailable in this worker");

    // v2 requires this subprotocol; v1 accepts it too. No application credentials are added.
    const socket = new WebSocketImpl(url, "xrpc.v1.json");
    const stream = new BrowserWebSocket(socket, maxBytes);
    return await new Promise<JsWebSocketHandle>((resolve, reject) => {
      const cleanup = (): void => {
        socket.removeEventListener("open", opened);
        socket.removeEventListener("error", failed);
        socket.removeEventListener("close", closedBeforeOpen);
      };
      const opened = (): void => {
        cleanup();
        resolve(stream);
      };
      const failed = (): void => {
        cleanup();
        reject(hostError("network", "WebSocket connection failed"));
      };
      const closedBeforeOpen = (): void => {
        cleanup();
        reject(hostError("network", "WebSocket closed before opening"));
      };
      socket.addEventListener("open", opened);
      socket.addEventListener("error", failed);
      socket.addEventListener("close", closedBeforeOpen);
    });
  } catch (e) {
    throw asHostError(e);
  }
}

export function createJsHost(deps: HostDeps): JsHostImports {
  const now = deps.now ?? (() => Date.now());
  /** The downloads in flight, by CID (see the module comment). */
  const downloads = new Map<string, Promise<BlobHandleValue>>();
  return {
    async http(method, url, account, path, headers, body, maxBytes, timeoutMs, redirects) {
      try {
        if (url !== undefined) {
          const req: AnonRequest = {
            method,
            url,
            headers: pairs(headers),
            body,
            maxBytes,
            timeoutMs,
            redirects,
          };
          return await fetchAnon(req, deps.fetch);
        }
        if (account === undefined || path === undefined) {
          throw hostError("other", "a request without a target");
        }
        if (method !== "GET" && method !== "POST") {
          throw hostError("blocked", `${method} is not sent to a user's PDS`);
        }
        return await deps.userPds({
          account,
          method,
          path,
          headers: pairs(headers),
          body,
          maxBytes,
          timeoutMs,
        });
      } catch (e) {
        throw asHostError(e);
      }
    },
    fetchBlob(pds, did, cid, maxBytes) {
      const running = downloads.get(cid);
      if (running) return running;
      const p: Promise<BlobHandleValue> = fetchBlob(
        { ...deps.fetch, store: deps.blobs },
        pds,
        did,
        cid,
        maxBytes,
      )
        .catch((e: unknown) => {
          throw asHostError(e);
        })
        .finally(() => {
          if (downloads.get(cid) === p) downloads.delete(cid);
        });
      downloads.set(cid, p);
      return p;
    },
    async blobOpen(cid) {
      try {
        const b = await deps.blobs.get(cid);
        return b ? { cid, size: b.size, locator: deps.blobs.locator(cid) } : null;
      } catch (e) {
        throw asHostError(e);
      }
    },
    async blobRead(cid, offset, len) {
      try {
        const b = await deps.blobs.get(cid);
        if (!b) throw hostError("storage", `blob ${cid} is not stored`);
        const n = Math.min(len, BLOB_READ_MAX_BYTES);
        return new Uint8Array(await b.slice(offset, offset + n).arrayBuffer());
      } catch (e) {
        throw asHostError(e);
      }
    },
    async blobDelete(cid) {
      try {
        await deps.blobs.delete(cid);
      } catch (e) {
        throw asHostError(e);
      }
    },
    websocket(url, maxBytes) {
      return openWebSocket(deps.webSocket ?? globalThis.WebSocket, url, maxBytes);
    },
    async lock(name, wait) {
      try {
        const tab = await acquireLock(deps.locks, name, wait);
        if (!tab || !deps.bridgeLock || !isBridgedLock(name)) return tab;
        let native: LockHandle | null;
        try {
          native = await takeBridgeLock(deps.bridgeLock, name, wait);
        } catch (e) {
          tab.release();
          throw e;
        }
        if (!native) {
          tab.release();
          return null;
        }
        const held = native;
        return {
          release() {
            held.release();
            tab.release();
          },
        };
      } catch (e) {
        throw asHostError(e);
      }
    },
    sleep(ms) {
      return new Promise((resolve) => setTimeout(resolve, Math.max(0, Math.min(ms, 2 ** 31 - 1))));
    },
    now,
    random(n) {
      const out = new Uint8Array(n);
      // getRandomValues takes at most 65,536 bytes per call
      for (let i = 0; i < n; i += 65536) crypto.getRandomValues(out.subarray(i, i + 65536));
      return out;
    },
    cache: deps.cache,
    state: deps.state,
  };
}
