// SPDX-License-Identifier: AGPL-3.0-or-later
// E2E BUILDS ONLY (`VITE_E2E=1`). Every import of this module sits behind the build flag
// (`import.meta.env.VITE_E2E === "1"`), so a release build contains none of it; the release
// bundle check (scripts/check-release-bundle.mjs, src/host/releaseBundle.test.ts) fails if any
// of its marker strings appears there.
//
// - Options come from `window.__LESSDUGIN_E2E__`, which the test harness sets before the page
//   loads (Playwright `addInitScript`): the devnet endpoints (ADR 0006; threat-model INFRA-9,
//   §6.2) and the parity time (engine-api.md §2.1). Nothing reads them from the URL.
// - The debug hook `window.__lessduginE2E` returns dto/1 JSON text exactly as the engine
//   serialises it (`callJson`, the bytes the CLI's `--json` prints) for the parity checks.

import type { EngineClient, EngineConfigInput } from "@lessdugin/engine/client";
import type { ListPostsQuery, ThreadOpts } from "@lessdugin/engine/types";

export interface E2EOptions {
  /** The devnet's PLC directory and PDS (http, loopback). */
  devnet?: { plc: string; pds: string };
  /** Direct PDS/PLC only, or explicitly configured local CORE+ services. */
  sources?: "direct" | "assisted";
  /** Local relay and Jetstream endpoints; empty arrays disable them. */
  relays?: string[];
  jetstream?: string[];
  /** Local optional indexers; null disables them. */
  constellation?: string | null;
  slingshot?: string | null;
  /** Parity mode: the fixed `now` of every view (ms since the epoch). */
  parityNowMs?: number;
  /** The engine worker runs one blocked `eval` after starting (WorkerInit `cspProbe`), so a
   * suite can check that worker violations reach its watcher. */
  cspProbe?: boolean;
}

export interface E2EDebug {
  /** Resolves once the engine worker runs. */
  ready(): Promise<void>;
  /** Any engine operation, as dto/1 JSON text. */
  callJson(op: string, args: unknown[]): Promise<string>;
  getThread(uri: string, opts?: Partial<ThreadOpts>): Promise<string>;
  listPosts(query: Partial<ListPostsQuery> & { forum: string }): Promise<string>;
  getPost(uri: string): Promise<string>;
  history(uri: string): Promise<string>;
  /** Sends every queued write now (`outbox_flush`); its `PumpReport` as JSON. */
  outboxFlush(): Promise<string>;
  writes(): Promise<string>;
  accounts(): Promise<string>;
  /** The options this page started with. */
  options: E2EOptions;
}

declare global {
  interface Window {
    __LESSDUGIN_E2E__?: unknown;
    __lessduginE2E?: E2EDebug;
  }
}

function loopback(u: unknown, protocols: readonly string[]): string | null {
  if (typeof u !== "string") return null;
  try {
    const url = new URL(u);
    const loop = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    return protocols.includes(url.protocol) && loop ? url.origin : null;
  } catch {
    return null;
  }
}

function loopbackHttp(u: unknown): string | null {
  return loopback(u, ["http:"]);
}

function loopbackList(raw: unknown, protocols: readonly string[]): string[] | undefined {
  if (!Array.isArray(raw)) return undefined;
  const urls = raw.map((u) => loopback(u, protocols));
  return urls.every((u): u is string => u !== null) ? urls : undefined;
}

/** The harness's options (validated: devnet endpoints must be loopback http). */
export function e2eOptions(raw: unknown = globalThis.window?.__LESSDUGIN_E2E__): E2EOptions {
  if (typeof raw !== "object" || raw === null) return {};
  const v = raw as {
    devnet?: { plc?: unknown; pds?: unknown };
    sources?: unknown;
    relays?: unknown;
    jetstream?: unknown;
    constellation?: unknown;
    slingshot?: unknown;
    parityNowMs?: unknown;
    cspProbe?: unknown;
  };
  const out: E2EOptions = {};
  const plc = loopbackHttp(v.devnet?.plc);
  const pds = loopbackHttp(v.devnet?.pds);
  if (plc && pds) out.devnet = { plc, pds };
  if (v.sources === "direct" || v.sources === "assisted") out.sources = v.sources;
  const relays = loopbackList(v.relays, ["http:"]);
  if (relays) out.relays = relays;
  const jetstream = loopbackList(v.jetstream, ["ws:"]);
  if (jetstream) out.jetstream = jetstream;
  for (const key of ["constellation", "slingshot"] as const) {
    if (v[key] === null) out[key] = null;
    else {
      const url = loopbackHttp(v[key]);
      if (url) out[key] = url;
    }
  }
  if (typeof v.parityNowMs === "number" && Number.isSafeInteger(v.parityNowMs)) {
    out.parityNowMs = v.parityNowMs;
  }
  if (v.cspProbe === true) out.cspProbe = true;
  return out;
}

/** The engine configuration the options ask for. */
export function e2eEngineConfig(o: E2EOptions): EngineConfigInput {
  const config: EngineConfigInput = {};
  if (o.devnet) config.network = { kind: "devnet", plc: o.devnet.plc, pds: o.devnet.pds };
  if (o.sources) config.sources = o.sources;
  if (o.relays) config.relays = o.relays;
  if (o.jetstream) config.jetstream = o.jetstream;
  if (o.constellation !== undefined) config.constellation = o.constellation;
  if (o.slingshot !== undefined) config.slingshot = o.slingshot;
  if (o.parityNowMs !== undefined) config.parityNowMs = o.parityNowMs;
  return config;
}

const DEFAULT_THREAD: ThreadOpts = { sort: "magic", unreadOnly: false };

/** Installs `window.__lessduginE2E`. */
export function installDebugHook(client: EngineClient, options: E2EOptions): E2EDebug {
  const hook: E2EDebug = {
    ready: async () => {
      await client.ready;
    },
    callJson: (op, args) => client.callJson(op, args),
    getThread: (uri, opts) => client.callJson("getThread", [uri, { ...DEFAULT_THREAD, ...opts }]),
    listPosts: (q) =>
      client.callJson("listPosts", [{ sort: "new", filter: "all", limit: 0, ...q }]),
    getPost: (uri) => client.callJson("getPost", [uri]),
    history: (uri) => client.callJson("history", [uri]),
    outboxFlush: () => client.callJson("outboxFlush", []),
    writes: () => client.callJson("writes", []),
    accounts: () => client.callJson("accounts", []),
    options,
  };
  window.__lessduginE2E = hook;
  return hook;
}
