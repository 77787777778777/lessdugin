// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The AuthBridge (threat-model AUTH-4, AUTH-8, SPA-13): runs on the MAIN thread, which holds the
// session, and performs the engine worker's `Target::UserPds` requests. The worker never sees a
// token or a DPoP key. Every request passes this allowlist first, whatever the engine asked:
//
// - `GET  /xrpc/com.atproto.server.getSession` (no query, no body);
// - `POST /xrpc/com.atproto.repo.{createRecord, putRecord, deleteRecord}` with a JSON body
//   whose `repo` is the account and whose `collection` is one of the 10 `NS` record
//   collections of this build;
// - `POST /xrpc/com.atproto.repo.applyWrites` likewise, for every op (`#create`, `#update`,
//   `#delete`), with at least one op;
// - `POST /xrpc/com.atproto.repo.uploadBlob` with a declared upload type (data-model §8.8).
//
// Anything else is refused locally (`blocked`). JSON bodies are sent re-serialised from the
// parsed value that was checked, so the PDS cannot read a different body than the one checked
// (duplicate keys, parser differentials). The request goes through the account's own session
// only (AUTH-8): the session must be bound to `req.account`.
//
// Sessions are pluggable (`SessionProvider`); the SPA plugs in one of:
// - hosted mode: `oauthSession(did, oauthSession.fetchHandler)` (the OAuth client's DPoP-bound
//   fetch, which resolves paths against the PDS bound at login);
// - `lessdugin web` mode: `bridgeSession({ did, token })`, which forwards to
//   `POST /bridge/xrpc/<nsid>?account=<did>` with the tab's bearer token (threat-model §7.4).
//
// The worker reads the public records of an account's own repo (`listRecords` and friends)
// anonymously from the account's PDS (worker-core.ts); those never reach this bridge.

import { flatHeaders, readCapped } from "./fetch.ts";
import {
  type AuthBridgePort,
  type HostErrorValue,
  type HttpResult,
  hostError,
  isHostError,
  type PdsRequest,
} from "./protocol.ts";
import type { Did } from "./types.ts";

/** The NSID prefix of this build (crates/lexicon `ns::NS`; `online.7778777.lessdugin` with the
 * final namespace, release 0.1). The worker refuses to start when the wasm's differs. */
export const NS = "online.7778777.lessdugin.temp";

/** The 10 record collections of `NS.authFull` (data-model.md §10.1), without the prefix. */
export const RECORD_COLLECTIONS = [
  "forum",
  "membership",
  "post",
  "comment",
  "revision",
  "opinion",
  "modAction",
  "wikiRevision",
  "sequence",
  "profile",
] as const;

/** Declared upload types (data-model.md §8.8). */
export const UPLOAD_TYPES = [
  "image/jpeg",
  "image/png",
  "image/webp",
  "image/gif",
  "audio/mp4",
  "video/mp4",
  "video/webm",
  "audio/mpeg",
  "audio/ogg",
  "application/pdf",
  "text/markdown",
] as const;

export const GET_SESSION = "com.atproto.server.getSession";
export const UPLOAD_BLOB = "com.atproto.repo.uploadBlob";
const RECORD_WRITES = new Set([
  "com.atproto.repo.createRecord",
  "com.atproto.repo.putRecord",
  "com.atproto.repo.deleteRecord",
]);
export const APPLY_WRITES = "com.atproto.repo.applyWrites";
const WRITE_OP_TYPES = new Set([
  "com.atproto.repo.applyWrites#create",
  "com.atproto.repo.applyWrites#update",
  "com.atproto.repo.applyWrites#delete",
]);

/** True for `<ns>.<collection>` of one of the record collections. */
export function isNsCollection(nsid: unknown, ns: string = NS): boolean {
  if (typeof nsid !== "string" || !nsid.startsWith(`${ns}.`)) return false;
  return (RECORD_COLLECTIONS as readonly string[]).includes(nsid.slice(ns.length + 1));
}

function refuse(why: string): never {
  throw hostError("blocked", `${why} (not on the write allowlist, threat-model AUTH-4)`);
}

/** The XRPC method of `path` (`/xrpc/<nsid>` with an optional query). */
export function xrpcMethod(path: string): { nsid: string; query: string } {
  const m = /^\/xrpc\/([A-Za-z0-9.-]+)(\?.*)?$/.exec(path);
  if (!m?.[1]) refuse(`path ${JSON.stringify(path.slice(0, 120))}`);
  return { nsid: m[1], query: m[2] ?? "" };
}

function header(req: PdsRequest, name: string): string | undefined {
  return req.headers.find(([k]) => k.toLowerCase() === name)?.[1];
}

/** A checked request: what is actually sent. */
export interface CheckedRequest {
  nsid: string;
  method: "GET" | "POST";
  /** `/xrpc/<nsid>` (no query: none of the allowed methods takes one). */
  path: string;
  contentType?: string;
  body?: Uint8Array;
}

/** Applies the allowlist to `req`; throws `blocked`. */
export function checkRequest(req: PdsRequest, ns: string = NS): CheckedRequest {
  const { nsid, query } = xrpcMethod(req.path);
  if (query !== "") refuse(`${nsid} with a query`);
  if (nsid === GET_SESSION) {
    if (req.method !== "GET" || req.body !== undefined) refuse(`${req.method} ${nsid}`);
    return { nsid, method: "GET", path: `/xrpc/${nsid}` };
  }
  if (req.method !== "POST") refuse(`${req.method} ${nsid}`);
  if (nsid === UPLOAD_BLOB) {
    const ct = header(req, "content-type")?.split(";")[0]?.trim().toLowerCase() ?? "";
    if (!(UPLOAD_TYPES as readonly string[]).includes(ct)) refuse(`${nsid} of type ${ct}`);
    return {
      nsid,
      method: "POST",
      path: `/xrpc/${nsid}`,
      contentType: ct,
      body: req.body ?? new Uint8Array(0),
    };
  }
  if (!RECORD_WRITES.has(nsid) && nsid !== APPLY_WRITES) refuse(`method ${nsid}`);
  const ct = header(req, "content-type")?.split(";")[0]?.trim().toLowerCase();
  if (ct !== "application/json") refuse(`${nsid} with content type ${ct}`);
  let body: unknown;
  try {
    body = JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(req.body ?? new Uint8Array(0)),
    );
  } catch {
    refuse(`${nsid} without a JSON body`);
  }
  if (typeof body !== "object" || body === null || Array.isArray(body)) {
    refuse(`${nsid} without a JSON object body`);
  }
  const b = body as Record<string, unknown>;
  if (b.repo !== req.account)
    refuse(`${nsid} to repo ${JSON.stringify(b.repo)}, not ${req.account}`);
  if (nsid === APPLY_WRITES) {
    const writes = b.writes;
    if (!Array.isArray(writes) || writes.length === 0) refuse(`${nsid} without writes`);
    for (const w of writes) {
      const op = (typeof w === "object" && w !== null ? w : {}) as Record<string, unknown>;
      if (!WRITE_OP_TYPES.has(op.$type as string)) refuse(`${nsid} op ${JSON.stringify(op.$type)}`);
      if (!isNsCollection(op.collection, ns)) {
        refuse(`${nsid} to collection ${JSON.stringify(op.collection)}`);
      }
    }
  } else if (!isNsCollection(b.collection, ns)) {
    refuse(`${nsid} to collection ${JSON.stringify(b.collection)}`);
  }
  return {
    nsid,
    method: "POST",
    path: `/xrpc/${nsid}`,
    contentType: "application/json",
    body: new TextEncoder().encode(JSON.stringify(body)),
  };
}

// ---------------------------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------------------------

export interface SessionFetchInit {
  method: "GET" | "POST";
  headers: Headers;
  body?: Uint8Array;
  signal: AbortSignal;
}

/** An account's authenticated session (OAuth or the `lessdugin web` bridge). */
export interface PdsSession {
  /** The account it is bound to. */
  readonly did: Did;
  /** Sends one checked XRPC request (`path` is `/xrpc/<nsid>`) to the account's PDS. */
  fetch(path: string, init: SessionFetchInit): Promise<Response>;
}

/** The SPA's sessions: `null` when the account has none (not logged in on this device). */
export interface SessionProvider {
  session(did: Did): Promise<PdsSession | null>;
}

/** Hosted mode: an OAuth session's DPoP-bound fetch handler (`@atproto/oauth-client-browser`
 * `OAuthSession.fetchHandler(pathname, init)`), which targets the PDS bound at login. */
export function oauthSession(
  did: Did,
  fetchHandler: (pathname: string, init: RequestInit) => Promise<Response>,
): PdsSession {
  return {
    did,
    fetch: (path, init) =>
      fetchHandler(path, {
        method: init.method,
        headers: init.headers,
        ...(init.body !== undefined ? { body: init.body as BodyInit } : {}),
        signal: init.signal,
      }),
  };
}

export interface BridgeSessionOptions {
  did: Did;
  /** The tab's bridge bearer token (`sessionStorage["lessdugin-bridge-token"]`). */
  token: string;
  /** Default `/bridge/xrpc` (same origin). */
  base?: string;
  fetch?: typeof fetch;
}

/** The header the `lessdugin web` bridge puts on answers it forwards from the PDS; an answer
 * without it is the bridge's own (crates/cli/src/web/server.rs). */
export const BRIDGE_UPSTREAM_HEADER = "x-lessdugin-upstream";

/** The host error of a bridge's own error answer `{"error": <engine error code>, "message"}`
 * (never a forwarded PDS answer): a 401 means the tab's bridge session is gone. */
export async function bridgeError(resp: Response): Promise<HostErrorValue> {
  let code = "";
  let message = "";
  try {
    const v = JSON.parse(await resp.text()) as { error?: unknown; message?: unknown };
    if (typeof v.error === "string") code = v.error;
    if (typeof v.message === "string") message = v.message.slice(0, 500);
  } catch {
    // not JSON: the status decides
  }
  const what = `lessdugin web bridge: ${message || `HTTP ${resp.status}`}`;
  if (resp.status === 401) return hostError("authRequired", `${what} (the bridge session ended)`);
  switch (code) {
    case "auth_required":
      return hostError("authRequired", what);
    case "blocked":
      return hostError("blocked", what);
    case "too_large":
      return hostError("tooLarge", what);
    case "unsupported":
      return hostError("unsupported", what);
    case "timeout":
      return hostError("timeout", what);
    case "network":
      return hostError(resp.status === 504 ? "timeout" : "network", what);
  }
  if (resp.status === 504) return hostError("timeout", what);
  if (resp.status === 502) return hostError("network", what);
  if (resp.status === 403) return hostError("blocked", what);
  if (resp.status === 413) return hostError("tooLarge", what);
  return hostError("other", what);
}

/** `lessdugin web` mode: `POST /bridge/xrpc/<nsid>?account=<did>` with the bearer token; the
 * CLI's keyring session signs it, after the bridge re-applies these checks (§7.4). The bridge
 * takes POST only, so `getSession` is sent as a POST too. Answers forwarded from the PDS
 * (`X-Lessdugin-Upstream: pds`) are returned as they are; the bridge's own error answers reject
 * with the matching host error, so a bridge 401 never reads as a PDS 401. */
export function bridgeSession(opts: BridgeSessionOptions): PdsSession {
  const base = opts.base ?? "/bridge/xrpc";
  const f = opts.fetch ?? globalThis.fetch.bind(globalThis);
  return {
    did: opts.did,
    async fetch(path, init) {
      const nsid = path.replace(/^\/xrpc\//, "");
      const headers = new Headers(init.headers);
      headers.set("authorization", `Bearer ${opts.token}`);
      if (init.body === undefined) headers.set("content-type", "application/json");
      const resp = await f(`${base}/${nsid}?account=${encodeURIComponent(opts.did)}`, {
        method: "POST",
        headers,
        body: (init.body ?? new TextEncoder().encode("{}")) as BodyInit,
        cache: "no-store",
        credentials: "omit",
        signal: init.signal,
      });
      if (resp.status >= 400 && resp.headers.get(BRIDGE_UPSTREAM_HEADER) !== "pds") {
        throw await bridgeError(resp);
      }
      return resp;
    },
  };
}

// ---------------------------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------------------------

export interface AuthBridgeOptions {
  /** The NSID prefix (default: this build's `NS`). */
  namespace?: string;
}

/** Performs the worker's `Target::UserPds` requests through the allowlist. Expose it to the
 * worker with `Comlink.proxy(bridge)` (client.ts does). */
export class AuthBridge implements AuthBridgePort {
  readonly namespace: string;

  constructor(
    private readonly sessions: SessionProvider,
    opts: AuthBridgeOptions = {},
  ) {
    this.namespace = opts.namespace ?? NS;
  }

  async request(req: PdsRequest): Promise<HttpResult> {
    const checked = checkRequest(req, this.namespace);
    const session = await this.sessions.session(req.account);
    if (!session) throw hostError("authRequired", `${req.account} has no session on this device`);
    if (session.did !== req.account) {
      // AUTH-8: never another account's session
      throw hostError("blocked", `the session for ${req.account} is bound to ${session.did}`);
    }
    const headers = new Headers();
    if (checked.contentType) headers.set("content-type", checked.contentType);
    const accept = header(req, "accept");
    if (accept) headers.set("accept", accept);
    const ctl = new AbortController();
    const timer = setTimeout(() => ctl.abort(), req.timeoutMs);
    try {
      let resp: Response;
      try {
        resp = await session.fetch(checked.path, {
          method: checked.method,
          headers,
          ...(checked.body !== undefined ? { body: checked.body } : {}),
          signal: ctl.signal,
        });
      } catch (e) {
        if (isHostError(e)) throw e;
        if (e instanceof DOMException && e.name === "AbortError") {
          throw hostError("timeout", `${checked.nsid}: no answer within ${req.timeoutMs} ms`);
        }
        // the request may have reached the PDS: the outbox treats this as ambiguous
        throw hostError("network", `${checked.nsid}: ${e instanceof Error ? e.message : e}`);
      }
      if (resp.type === "opaqueredirect" || (resp.status >= 300 && resp.status < 400)) {
        throw hostError("blocked", `${checked.nsid}: the PDS redirected (never followed)`);
      }
      const body = await readCapped(resp.body, req.maxBytes);
      return { status: resp.status, headers: flatHeaders(resp.headers), body };
    } finally {
      clearTimeout(timer);
    }
  }
}
