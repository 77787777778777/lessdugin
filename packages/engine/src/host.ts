// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The JS host object of the engine worker: what `new WasmEngine(configJson, host)` calls for the
// core's `Host` trait (crates/wasm/src/host.rs `JsHostImports`, sync.md §12). The pieces:
//
// - `http(...)`: `Target::Anon` requests through `fetchAnon` (fetch.ts); `Target::UserPds`
//   requests through `userPds` (the worker: the main thread's AuthBridge);
// - `fetchBlob(...)`: blobs.ts;
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
  /** `lessdugin web` mode: `outbox:<did>` locks are also taken natively (`POST /bridge/lock`). */
  bridgeLock?: BridgeLockOptions | undefined;
  /** Default `Date.now`. */
  now?: () => number;
}

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

export function createJsHost(deps: HostDeps): JsHostImports {
  const now = deps.now ?? (() => Date.now());
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
    async fetchBlob(pds, did, cid, maxBytes) {
      try {
        return await fetchBlob({ ...deps.fetch, store: deps.blobs }, pds, did, cid, maxBytes);
      } catch (e) {
        throw asHostError(e);
      }
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
