// SPDX-License-Identifier: AGPL-3.0-or-later
// `lessdugin web` mode (threat-model §7): the CLI keeps the session; the tab holds only a
// per-launch bridge session token, in sessionStorage.
//
// - Startup, before the router mounts: the `#lessdugin-cap=` capability (already removed from
//   the address bar by lib/bootParams.ts) is exchanged once at `POST /bridge/session` for the
//   token (§7.2). Without a capability the tab reuses its token (a reload, or the return from a
//   sign-in). A refused capability, or no token at all, stops the app with the §7.2 message.
// - Writes: `Target::UserPds` requests go through `POST /bridge/xrpc/<nsid>?account=<did>`
//   (`bridgeSession`), the `state` store through `POST /bridge/kv` (the engine worker).
// - Sign-in: `POST /bridge/login {actor}` starts the CLI's loopback OAuth and answers the
//   authorization URL; the CLI's callback sends the tab back to `/#/` (or
//   `/#/login?error=<code>`).

import type { SessionProvider } from "@lessdugin/engine/client";
import { bridgeSession } from "@lessdugin/engine/client";
import type { EngineError } from "@lessdugin/engine/types";

/** sessionStorage key of the tab's bridge token (§7.2). */
export const BRIDGE_TOKEN_KEY = "lessdugin-bridge-token";

const TOKEN_RE = /^[A-Za-z0-9_-]{43}$/;

/** The §7.2 text for a capability that was refused, or a tab without a session. */
export const LINK_USED_MESSAGE =
  "This link was already used or has expired; open a fresh link from the terminal (press Enter where `lessdugin web` runs).";

/** The app cannot run: the tab has no bridge session. */
export class BridgeLinkError extends Error {
  override readonly name = "BridgeLinkError";
  constructor() {
    super(LINK_USED_MESSAGE);
  }
}

function storageGet(storage: Storage | undefined, key: string): string | null {
  try {
    return storage?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function storageSet(storage: Storage | undefined, key: string, value: string): void {
  try {
    storage?.setItem(key, value);
  } catch {
    // sessionStorage unavailable: the token lives as long as this page
  }
}

function sessionStorageOrUndefined(): Storage | undefined {
  try {
    return globalThis.sessionStorage;
  } catch {
    return undefined;
  }
}

/** Exchanges `cap` for a bridge token, or returns the tab's stored token. */
export async function bridgeToken(
  cap: string | null,
  storage: Storage | undefined = sessionStorageOrUndefined(),
  fetchFn: typeof fetch = globalThis.fetch.bind(globalThis),
): Promise<string> {
  if (cap !== null) {
    const resp = await fetchFn("/bridge/session", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ cap }),
      cache: "no-store",
      credentials: "omit",
    });
    if (resp.status === 403 || resp.status === 401) throw new BridgeLinkError();
    if (!resp.ok) throw new Error(`lessdugin web: /bridge/session answered HTTP ${resp.status}`);
    const { token } = (await resp.json()) as { token?: unknown };
    if (typeof token !== "string" || !TOKEN_RE.test(token)) {
      throw new Error("lessdugin web: /bridge/session answered without a token");
    }
    storageSet(storage, BRIDGE_TOKEN_KEY, token);
    return token;
  }
  const stored = storageGet(storage, BRIDGE_TOKEN_KEY);
  if (stored !== null && TOKEN_RE.test(stored)) return stored;
  throw new BridgeLinkError();
}

/** Every account's writes go through the bridge (the CLI checks that it is logged in). */
export function bridgeSessions(token: string): SessionProvider {
  return { session: async (did) => bridgeSession({ did, token }) };
}

function engineError(code: EngineError["code"], message: string): EngineError {
  return { code, message, retryable: false };
}

/** `POST /bridge/login {actor}`: the authorization URL of the CLI's loopback OAuth. */
export async function bridgeLogin(
  token: string,
  actor: string,
  fetchFn: typeof fetch = globalThis.fetch.bind(globalThis),
): Promise<URL> {
  const resp = await fetchFn("/bridge/login", {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ actor }),
    cache: "no-store",
    credentials: "omit",
  });
  const text = await resp.text();
  let body: { url?: unknown; error?: unknown; message?: unknown } = {};
  try {
    body = JSON.parse(text) as typeof body;
  } catch {
    // the status decides
  }
  if (resp.status === 401) {
    throw engineError("auth_required", `The lessdugin web session ended. ${LINK_USED_MESSAGE}`);
  }
  if (!resp.ok) {
    const message = typeof body.message === "string" ? body.message.slice(0, 500) : "";
    const code = typeof body.error === "string" ? (body.error as EngineError["code"]) : "internal";
    throw engineError(code, message || `lessdugin web: sign-in failed (HTTP ${resp.status})`);
  }
  if (typeof body.url !== "string") throw engineError("internal", "no authorization URL");
  const url = new URL(body.url);
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw engineError("blocked", `refusing an authorization URL with scheme ${url.protocol}`);
  }
  return url;
}
