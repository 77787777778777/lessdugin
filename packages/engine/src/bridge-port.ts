// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The link between the engine worker and the main thread's AuthBridge (threat-model AUTH-4,
// SPA-13).
//
// The worker parses every byte that comes from the network, so the main thread treats it as
// untrusted. It exposes no object to it: a Comlink proxy would let the worker send GET, SET and
// APPLY messages along any property path of the exposed object (`bridge.namespace = …`, or
// `__proto__.__proto__.x = …` to pollute the main thread's `Object.prototype`, next to the OAuth
// client). Instead the worker gets one `MessagePort` with a fixed message schema:
//
//   worker → main   {t: "req", id, req}    an authenticated request (`PdsRequest`), which only
//                                          `AuthBridge.request` handles
//                   {t: "event", json}     an engine event (its dto/1 JSON text)
//   main → worker   {t: "res", id, ok: true, res}        the `HttpResult`
//                   {t: "res", id, ok: false, error}     a `HostErrorValue`
//
// The main thread checks the type of every field and rebuilds the request from those values
// before the allowlist sees it; any other message is dropped. `id` is a non-negative safe
// integer chosen by the worker.

import {
  type AuthBridgePort,
  type HostErrorKind,
  type HostErrorValue,
  type HttpResult,
  hostError,
  isHostError,
  type PdsRequest,
} from "./protocol.ts";

/** What the main thread does with the worker's messages. */
export interface BridgeHandlers {
  /** An authenticated request (already shape-checked); normally `AuthBridge.request`. */
  request(req: PdsRequest): Promise<HttpResult>;
  /** An engine event's JSON text. */
  event(json: string): void;
}

/** The `MessagePort` surface used here (a DOM `MessagePort` or node's). */
export interface PortLike {
  postMessage(message: unknown): void;
  addEventListener(type: "message", listener: (ev: MessageEvent) => void): void;
  removeEventListener(type: "message", listener: (ev: MessageEvent) => void): void;
  start?(): void;
  close(): void;
}

/** Upper bounds for the fields of a request from the worker (the engine's own limits are far
 * below them; anything larger is not the engine's). */
const MAX_PATH = 8192;
const MAX_ACCOUNT = 2048;
const MAX_HEADERS = 32;
const MAX_HEADER_LEN = 8192;
const MAX_TIMEOUT_MS = 600_000;

const HOST_ERROR_KINDS: ReadonlySet<string> = new Set<HostErrorKind>([
  "network",
  "timeout",
  "tooLarge",
  "blocked",
  "cors",
  "authRequired",
  "storage",
  "unsupported",
  "cidMismatch",
  "httpStatus",
  "other",
]);

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function isId(v: unknown): v is number {
  return typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
}

/** A `PdsRequest` rebuilt from `v`'s checked fields, or `null` if `v` is not one. */
export function parsePdsRequest(v: unknown): PdsRequest | null {
  if (!isRecord(v)) return null;
  const { account, method, path, headers, body, maxBytes, timeoutMs } = v;
  if (typeof account !== "string" || account.length === 0 || account.length > MAX_ACCOUNT) {
    return null;
  }
  if (method !== "GET" && method !== "POST") return null;
  if (typeof path !== "string" || path.length > MAX_PATH) return null;
  if (!Array.isArray(headers) || headers.length > MAX_HEADERS) return null;
  const hs: [string, string][] = [];
  for (const h of headers) {
    if (!Array.isArray(h) || h.length !== 2) return null;
    const [k, val] = h as unknown[];
    if (typeof k !== "string" || typeof val !== "string") return null;
    if (k.length > MAX_HEADER_LEN || val.length > MAX_HEADER_LEN) return null;
    hs.push([k, val]);
  }
  if (body !== undefined && !(body instanceof Uint8Array)) return null;
  if (!isId(maxBytes)) return null;
  if (!isId(timeoutMs) || timeoutMs > MAX_TIMEOUT_MS) return null;
  return {
    account,
    method,
    path,
    headers: hs,
    // a copy that owns its buffer: nothing the worker shares stays reachable
    ...(body !== undefined ? { body: new Uint8Array(body) } : {}),
    maxBytes,
    timeoutMs,
  };
}

/** `e` as a plain host error value (only its known fields, as strings and numbers). */
function plainHostError(e: unknown): HostErrorValue {
  if (isHostError(e) && HOST_ERROR_KINDS.has(e.kind)) {
    const extra: { status?: number; name?: string } = {};
    if (typeof e.status === "number") extra.status = e.status;
    if (typeof e.name === "string") extra.name = e.name;
    return hostError(e.kind, String(e.message).slice(0, 2000), extra);
  }
  return hostError("other", e instanceof Error ? e.message : String(e));
}

/** Main thread: serves the worker's end of `port`. Returns a function that stops serving. */
export function serveBridgePort(port: PortLike, handlers: BridgeHandlers): () => void {
  let open = true;
  const reply = (msg: unknown) => {
    if (!open) return;
    try {
      port.postMessage(msg);
    } catch {
      // the worker is gone
    }
  };
  const onMessage = (ev: MessageEvent): void => {
    const m: unknown = ev.data;
    if (!isRecord(m)) return;
    if (m.t === "event") {
      if (typeof m.json === "string") handlers.event(m.json);
      return;
    }
    if (m.t !== "req" || !isId(m.id)) return;
    const id = m.id;
    const req = parsePdsRequest(m.req);
    if (!req) {
      reply({ t: "res", id, ok: false, error: hostError("blocked", "malformed bridge request") });
      return;
    }
    let p: Promise<HttpResult>;
    try {
      p = handlers.request(req);
    } catch (e) {
      p = Promise.reject(e);
    }
    p.then(
      (res) =>
        reply({
          t: "res",
          id,
          ok: true,
          res: { status: res.status, headers: [...res.headers], body: res.body },
        }),
      (e: unknown) => reply({ t: "res", id, ok: false, error: plainHostError(e) }),
    );
  };
  port.addEventListener("message", onMessage);
  port.start?.();
  return () => {
    open = false;
    port.removeEventListener("message", onMessage);
    port.close();
  };
}

/** The worker's end: an `AuthBridgePort` and the event sender. */
export interface BridgeLink {
  bridge: AuthBridgePort;
  emit(json: string): void;
  close(): void;
}

function parseResult(v: unknown): HttpResult | null {
  if (!isRecord(v)) return null;
  const { status, headers, body } = v;
  if (typeof status !== "number" || !Array.isArray(headers) || !(body instanceof Uint8Array)) {
    return null;
  }
  if (!headers.every((h) => typeof h === "string")) return null;
  return { status, headers: headers as string[], body };
}

/** Worker: connects to the main thread's `serveBridgePort` over `port`. */
export function connectBridgePort(port: PortLike): BridgeLink {
  let next = 0;
  let closed = false;
  const pending = new Map<
    number,
    { resolve: (r: HttpResult) => void; reject: (e: HostErrorValue) => void }
  >();
  const onMessage = (ev: MessageEvent): void => {
    const m: unknown = ev.data;
    if (!isRecord(m) || m.t !== "res" || !isId(m.id)) return;
    const p = pending.get(m.id);
    if (!p) return;
    pending.delete(m.id);
    if (m.ok === true) {
      const res = parseResult(m.res);
      if (res) p.resolve(res);
      else p.reject(hostError("other", "malformed AuthBridge answer"));
    } else {
      p.reject(plainHostError(m.error));
    }
  };
  port.addEventListener("message", onMessage);
  port.start?.();
  return {
    bridge: {
      request(req: PdsRequest): Promise<HttpResult> {
        if (closed) return Promise.reject(hostError("other", "the AuthBridge link is closed"));
        const id = next++;
        return new Promise<HttpResult>((resolve, reject) => {
          pending.set(id, { resolve, reject });
          try {
            port.postMessage({ t: "req", id, req });
          } catch (e) {
            pending.delete(id);
            reject(plainHostError(e));
          }
        });
      },
    },
    emit(json: string): void {
      if (closed) return;
      port.postMessage({ t: "event", json });
    },
    close(): void {
      if (closed) return;
      closed = true;
      port.removeEventListener("message", onMessage);
      port.close();
      for (const p of pending.values()) p.reject(hostError("other", "the AuthBridge link closed"));
      pending.clear();
    },
  };
}
