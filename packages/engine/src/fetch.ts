// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Anonymous HTTP for the engine worker (`Target::Anon`, sync.md §12.1): `fetch` with
//
// - `cache: "no-store"`: never answered from or stored in an HTTP cache (heads and listings
//   must be fresh);
// - `credentials: "omit"`, `referrerPolicy: "no-referrer"`, and only the CORS-safelisted headers
//   the core sets (`accept`, `content-type`), so cross-origin GETs need no preflight (INFRA-10);
// - the byte cap counted while streaming, after decompression, aborting at the first byte over
//   it; `Content-Length` is never trusted (INFRA-3);
// - the total timeout (an `AbortController`);
// - the browser URL policy of threat-model §5.6.2 (the browser cannot see addresses): `https`
//   only (`http` only in devnet mode), no userinfo, no IP literal, no `localhost`, `*.localhost`,
//   `*.local`, `*.internal`, `*.home.arpa` or single-label host (devnet: loopback allowed);
// - redirects: none for `redirects = 0` (a redirect answers status 302 without headers: the
//   browser hides the real status and `Location`); otherwise the browser follows them and the
//   final URL must pass the policy again (the browser does not expose the hop count).
//
// A failed cross-origin request (a `TypeError`) cannot be told apart from a network error by
// `fetch` itself; for GETs a `no-cors` probe of the same URL decides: reachable → `cors`,
// unreachable → `network`. Offline (`navigator.onLine === false`) is always `network`.

import { type AnonRequest, type HttpResult, hostError, isHostError } from "./protocol.ts";

/** Hostnames that never belong to a public service (threat-model §5.6.2, browser rules). */
const LOCAL_SUFFIXES = [".localhost", ".local", ".internal", ".home.arpa"];

export interface UrlPolicy {
  /** Devnet mode (ADR 0006): `http` and loopback hosts are allowed. */
  devnet: boolean;
}

function isIpLiteral(host: string): boolean {
  return host.startsWith("[") || /^[0-9.]+$/.test(host) || /^0x/i.test(host);
}

function isLoopback(host: string): boolean {
  return host === "localhost" || host === "[::1]" || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host);
}

/** Parses `raw` and applies the browser URL policy; throws a `blocked` host error. */
export function checkUrl(raw: string, policy: UrlPolicy): URL {
  let u: URL;
  try {
    u = new URL(raw);
  } catch {
    throw hostError("blocked", `not an absolute URL: ${JSON.stringify(raw.slice(0, 200))}`);
  }
  const scheme = u.protocol;
  if (scheme !== "https:" && !(policy.devnet && scheme === "http:")) {
    throw hostError("blocked", `scheme ${scheme} is not allowed`);
  }
  if (u.username !== "" || u.password !== "") {
    throw hostError("blocked", "URLs with userinfo are not allowed");
  }
  const host = u.hostname.toLowerCase();
  if (policy.devnet && isLoopback(host)) return u;
  if (isIpLiteral(host)) throw hostError("blocked", `IP literal host ${host}`);
  if (host === "localhost" || LOCAL_SUFFIXES.some((s) => host.endsWith(s))) {
    throw hostError("blocked", `local host name ${host}`);
  }
  if (!host.includes(".")) throw hostError("blocked", `single-label host ${host}`);
  return u;
}

/** Request headers an anonymous request may carry (CORS-safelisted; the core sets no others). */
const ANON_HEADERS = new Set(["accept", "content-type", "accept-language"]);

/** Reads `body` to the end, failing with `tooLarge` past `max` bytes (and cancelling it). */
export async function readCapped(
  body: ReadableStream<Uint8Array> | null,
  max: number,
): Promise<Uint8Array> {
  if (body === null) return new Uint8Array(0);
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let n = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    n += value.byteLength;
    if (n > max) {
      await reader.cancel().catch(() => {});
      throw hostError("tooLarge", `response larger than ${max} bytes`);
    }
    chunks.push(value);
  }
  if (chunks.length === 1 && chunks[0]) return chunks[0];
  const out = new Uint8Array(n);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.byteLength;
  }
  return out;
}

/** Response headers, flattened and lowercased. */
export function flatHeaders(h: Headers): string[] {
  const out: string[] = [];
  h.forEach((value, name) => {
    out.push(name.toLowerCase(), value);
  });
  return out;
}

export interface FetchDeps {
  fetch: typeof fetch;
  policy: UrlPolicy;
  /** `navigator.onLine`, when known. */
  onLine?: () => boolean;
}

/** Classifies a `fetch` rejection (see the module comment). */
async function failure(e: unknown, u: URL, req: AnonRequest, deps: FetchDeps): Promise<unknown> {
  if (isHostError(e)) return e;
  if (e instanceof DOMException && e.name === "AbortError") {
    return hostError("timeout", `no answer from ${u.host} within ${req.timeoutMs} ms`);
  }
  const message = e instanceof Error ? e.message : String(e);
  if (deps.onLine?.() === false) return hostError("network", `offline: ${message}`);
  if (req.method !== "GET") return hostError("cors", `${u.host}: ${message}`);
  try {
    const probe = new AbortController();
    const t = setTimeout(() => probe.abort(), Math.min(req.timeoutMs, 10_000));
    try {
      await deps.fetch(u.href, {
        method: "GET",
        mode: "no-cors",
        cache: "no-store",
        credentials: "omit",
        referrerPolicy: "no-referrer",
        redirect: "manual",
        signal: probe.signal,
      });
    } finally {
      clearTimeout(t);
    }
    return hostError("cors", `${u.host} refused a cross-origin request`);
  } catch {
    return hostError("network", `${u.host}: ${message}`);
  }
}

/** One anonymous request under the rules above. */
export async function fetchAnon(req: AnonRequest, deps: FetchDeps): Promise<HttpResult> {
  const u = checkUrl(req.url, deps.policy);
  const headers = new Headers();
  for (const [k, v] of req.headers) {
    if (ANON_HEADERS.has(k.toLowerCase())) headers.set(k, v);
  }
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), req.timeoutMs);
  try {
    let resp: Response;
    try {
      resp = await deps.fetch(u.href, {
        method: req.method,
        headers,
        ...(req.body !== undefined ? { body: req.body as BodyInit } : {}),
        cache: "no-store",
        credentials: "omit",
        mode: "cors",
        referrerPolicy: "no-referrer",
        redirect: req.redirects > 0 ? "follow" : "manual",
        signal: ctl.signal,
      });
    } catch (e) {
      throw await failure(e, u, req, deps);
    }
    if (resp.type === "opaqueredirect") {
      return { status: 302, headers: [], body: new Uint8Array(0) };
    }
    if (resp.redirected && resp.url !== "") checkUrl(resp.url, deps.policy);
    let body: Uint8Array;
    try {
      body = await readCapped(resp.body, req.maxBytes);
    } catch (e) {
      // the server answered: a failure now is the transfer's, never CORS
      if (isHostError(e)) throw e;
      if (e instanceof DOMException && e.name === "AbortError") {
        throw hostError("timeout", `${u.host}: the answer did not finish in ${req.timeoutMs} ms`);
      }
      throw hostError("network", `${u.host}: ${e instanceof Error ? e.message : String(e)}`);
    }
    return { status: resp.status, headers: flatHeaders(resp.headers), body };
  } finally {
    clearTimeout(timer);
  }
}
