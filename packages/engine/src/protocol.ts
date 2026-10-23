// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Shapes shared by the main thread (`client.ts`, `auth-bridge.ts`), the engine worker
// (`worker.ts`) and the wasm host (`host.ts`). Nothing here touches the DOM or the network.

import type { Did, EngineEvent } from "./types.ts";

// ---------------------------------------------------------------------------------------------
// Engine configuration (crates/wasm/src/config.rs `ConfigInput`)
// ---------------------------------------------------------------------------------------------

/** The engine configuration the worker passes to `new WasmEngine(json, host)`. Every field is
 * optional; absent fields keep the engine's production defaults (`engine-api.md` §2). */
export interface EngineConfigInput {
  /** Production (default) or a local devnet (ADR 0006). The worker accepts `devnet` only in
   * development and e2e builds (threat-model INFRA-9). */
  network?: { kind: "production" } | { kind: "devnet"; plc: string; pds: string };
  sources?: "direct" | "assisted";
  verify?: "pds" | "proof";
  /** DoH resolvers (`https://…/dns-query`-style JSON endpoints). */
  doh?: string[];
  plc?: string;
  relays?: string[];
  jetstream?: string[];
  /** `null` disables it. */
  constellation?: string | null;
  /** `null` disables it. */
  slingshot?: string | null;
  indexers?: string[];
  /** Parity mode (`engine-api.md` §2.1). Test-only: the worker accepts it only in e2e builds
   * (`VITE_E2E=1`); release builds drop the code path. */
  parityNowMs?: number;
}

// ---------------------------------------------------------------------------------------------
// Host errors (crates/wasm/src/host.rs `host_error`)
// ---------------------------------------------------------------------------------------------

/** The `HostError` variants, camelCased. */
export type HostErrorKind =
  | "network"
  | "timeout"
  | "tooLarge"
  | "blocked"
  | "cors"
  | "authRequired"
  | "storage"
  | "unsupported"
  | "cidMismatch"
  | "httpStatus"
  | "other";

/** What the JS host rejects with; the wasm side maps it to `HostError`. */
export interface HostErrorValue {
  kind: HostErrorKind;
  message: string;
  /** `httpStatus` only. */
  status?: number;
  /** `httpStatus`: the XRPC error name from the body, if any. */
  name?: string;
}

export function hostError(
  kind: HostErrorKind,
  message: string,
  extra: { status?: number; name?: string } = {},
): HostErrorValue {
  return { kind, message, ...extra };
}

export function isHostError(e: unknown): e is HostErrorValue {
  return (
    typeof e === "object" &&
    e !== null &&
    typeof (e as { kind?: unknown }).kind === "string" &&
    typeof (e as { message?: unknown }).message === "string"
  );
}

// ---------------------------------------------------------------------------------------------
// HTTP between the host, the fetcher and the AuthBridge
// ---------------------------------------------------------------------------------------------

/** A response as the wasm host reads it: headers flattened `[name, value, …]`, names
 * lowercased. */
export interface HttpResult {
  status: number;
  headers: string[];
  body: Uint8Array;
}

/** An anonymous request (`Target::Anon`). */
export interface AnonRequest {
  method: string;
  url: string;
  /** `[name, value]` pairs. */
  headers: [string, string][];
  body?: Uint8Array | undefined;
  maxBytes: number;
  timeoutMs: number;
  /** 0: never follow a redirect. */
  redirects: number;
}

/** A `Target::UserPds` request the worker forwards to the main thread's AuthBridge. */
export interface PdsRequest {
  account: Did;
  method: "GET" | "POST";
  /** Path and query, `/xrpc/<nsid>?…`. */
  path: string;
  headers: [string, string][];
  body?: Uint8Array | undefined;
  maxBytes: number;
  timeoutMs: number;
}

/** The AuthBridge as the worker sees it (a Comlink proxy of `AuthBridge.request`). */
export interface AuthBridgePort {
  request(req: PdsRequest): Promise<HttpResult>;
}

// ---------------------------------------------------------------------------------------------
// Worker start-up
// ---------------------------------------------------------------------------------------------

/** Where the durable `state` store lives. */
export type StateStoreConfig =
  /** IndexedDB on this origin (hosted site). */
  | { kind: "indexeddb" }
  /** `lessdugin web`: the native `state.sqlite` through `POST /bridge/kv` (threat-model §7.4). */
  | {
      kind: "bridge";
      endpoint: string;
      token: string;
      /** `POST /bridge/lock` (default): the native `outbox:<did>` lock, taken after the tab's
       * Web Lock (threat-model §7.4). */
      lockEndpoint?: string;
    }
  /** In memory, lost with the worker (tests, previews). */
  | { kind: "memory" };

export interface WorkerInit {
  config: EngineConfigInput;
  /** The NSID prefix the main thread's AuthBridge allows; the worker refuses to start when the
   * wasm build's differs. */
  namespace: string;
  /** IndexedDB database name prefix (default `lessdugin`); stores are `<prefix>-cache`,
   * `<prefix>-state` and `<prefix>-blobs`. */
  dbPrefix?: string;
  /** Default `{ kind: "indexeddb" }`. */
  state?: StateStoreConfig;
  /** Default `true`: take part in the leader election (only the leader runs `outboxPump` and
   * `liveTail`). `false`: never lead (embedded or test workers). */
  elect?: boolean;
  /** E2E builds only (ignored elsewhere): after starting, the worker runs one `eval`, which the
   * CSP blocks, so a suite can check that a violation inside the worker reaches its watcher. */
  cspProbe?: boolean;
}

/** What `init` reports. */
export interface WorkerInfo {
  version: string;
  namespace: string;
  /** This worker's id on the change channel. */
  id: string;
}

// ---------------------------------------------------------------------------------------------
// The change channel between tabs (`sync.md` §10.6)
// ---------------------------------------------------------------------------------------------

/** BroadcastChannel name for engine events between tabs. */
export const CHANGES_CHANNEL = "lessdugin-changes";
/** The Web Lock held by the leader tab for its lifetime. It is also the Web Lock of the host
 * lock name `sync-leader` (`webLockName("sync-leader")`, sync.md §10.6): one leader per
 * origin, whichever asks. The worker answers its own engine's `sync-leader` request at once
 * while it leads (Web Locks are not re-entrant, worker-core.ts). */
export const LEADER_LOCK = "lessdugin-sync-leader";

export type ChannelMessage =
  /** An engine event of another tab (`changed` and `write` events are relayed). */
  | { type: "event"; from: string; event: EngineEvent }
  /** Another tab queued a write: the leader wakes its outbox pump. */
  | { type: "wake"; from: string };
