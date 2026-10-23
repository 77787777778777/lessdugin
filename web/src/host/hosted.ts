// SPDX-License-Identifier: AGPL-3.0-or-later
// Hosted-mode sign-in: `@atproto/oauth-client-browser` (pinned, synthesis §6.6) on the main
// thread, which keeps the session; the engine worker never sees a token or a DPoP key
// (threat-model SPA-13, AUTH-4). Loaded only when an OAuth session is needed.
//
// - `response_mode=query` (AUTH-5, data-model.md §10.5): the callback arrives as `?code&state&iss`
//   and is read and removed before the hash router mounts (lib/bootParams.ts, SPA-8); this
//   module never touches the address bar.
// - The engine resolves the user's input first (DoH, both directions) and the library is asked
//   for the DID only; its own handle resolver is disabled.
// - DPoP keys are WebCrypto keys generated non-extractable and kept in the library's IndexedDB
//   database (`@atproto-oauth-client`), AUTH-6. The library refreshes tokens under its own Web
//   Lock (`@atproto-oauth-client-<did>`), which serialises refreshes across tabs as AUTH-6
//   requires; its name is not configurable.
// - Scopes: data-model.md §10.3 (host/scopes.ts), never `transition:generic`.
// - Client: on the hosted site, the web client metadata next to the app
//   (`oauth/client-metadata.json`, redirect to the app's directory); on a loopback http origin
//   (a local preview or the e2e harness) the atproto loopback client
//   (`http://localhost?redirect_uri=…&scope=…`), which needs no metadata document.

import {
  type AuthorizeOptions,
  BrowserOAuthClient,
  type HandleResolver,
  type OAuthSession,
} from "@atproto/oauth-client-browser";
import type { PdsSession, SessionProvider } from "@lessdugin/engine/client";
import { NS, oauthSession } from "@lessdugin/engine/client";
import { scopeCandidates, supersetScope } from "./scopes.ts";

type RedirectUri = NonNullable<AuthorizeOptions["redirect_uri"]>;

/** RFC 3986 percent-encoding, unreserved characters kept (data-model.md §10.4 `pct`). */
export function pct(s: string): string {
  return encodeURIComponent(s).replace(
    /[!'()*]/g,
    (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

export interface ClientChoice {
  clientId: string;
  redirectUri: string;
  loopback: boolean;
}

/** The OAuth client for a page at `base` (`document.baseURI`). */
export function chooseClient(base: string, ns: string = NS): ClientChoice {
  const dir = new URL("./", base);
  dir.search = "";
  dir.hash = "";
  const host = dir.hostname;
  if (dir.protocol === "http:" && (host === "127.0.0.1" || host === "[::1]")) {
    const redirectUri = dir.href;
    return {
      clientId: `http://localhost?redirect_uri=${pct(redirectUri)}&scope=${pct(supersetScope(ns))}`,
      redirectUri,
      loopback: true,
    };
  }
  if (dir.protocol === "http:" && host === "localhost") {
    // The loopback redirect must be an IP literal, and storage is per origin: the callback
    // would land on another origin than the one that started the login.
    throw new Error(
      `Sign-in needs the IP address: open http://127.0.0.1${dir.port ? `:${dir.port}` : ""}${dir.pathname} instead.`,
    );
  }
  return {
    clientId: new URL("oauth/client-metadata.json", dir).href,
    redirectUri: dir.href,
    loopback: false,
  };
}

/** The engine resolved the input already: the library is only ever given DIDs, and the handle
 * in a DID document is left unverified (shown by the engine, not the library). */
const NO_HANDLE_RESOLUTION: HandleResolver = { resolve: async () => null };

export interface HostedOAuthOptions {
  /** Devnet (e2e builds only): http authorization servers and this PLC directory. */
  devnet?: { plc: string } | undefined;
}

/** What a finished callback yields for `addAccount`. */
export interface OAuthLogin {
  did: string;
  /** The PDS the session talks to (the token's `aud`). */
  pds: string;
  /** The authorization server (the token's `iss`). */
  issuer: string;
  /** Where the user was when they started (a `#/…` route), if known. */
  returnTo: string | null;
  /** The DID the user asked to sign in as. */
  intended: string | null;
}

function oauthErrorCode(e: unknown): string | undefined {
  if (typeof e !== "object" || e === null) return undefined;
  const v = (e as { error?: unknown }).error;
  return typeof v === "string" ? v : undefined;
}

interface AppState {
  returnTo?: string;
  intended?: string;
}

function parseAppState(s: string | null | undefined): AppState {
  if (!s) return {};
  try {
    const v = JSON.parse(s) as Record<string, unknown>;
    const out: AppState = {};
    if (typeof v.returnTo === "string" && /^#\/[^\s]*$/.test(v.returnTo)) out.returnTo = v.returnTo;
    if (typeof v.intended === "string" && v.intended.startsWith("did:")) out.intended = v.intended;
    return out;
  } catch {
    return {};
  }
}

export class HostedOAuth implements SessionProvider {
  private readonly sessions = new Map<string, Promise<OAuthSession>>();

  private constructor(
    private readonly client: BrowserOAuthClient,
    readonly choice: ClientChoice,
  ) {}

  static async create(
    opts: HostedOAuthOptions = {},
    base = document.baseURI,
  ): Promise<HostedOAuth> {
    const choice = chooseClient(base);
    const client = await BrowserOAuthClient.load({
      clientId: choice.clientId,
      responseMode: "query",
      handleResolver: NO_HANDLE_RESOLUTION,
      // Only e2e builds may talk http to an authorization server (the devnet).
      ...(opts.devnet && import.meta.env.VITE_E2E === "1"
        ? { allowHttp: true, plcDirectoryUrl: opts.devnet.plc }
        : {}),
    });
    return new HostedOAuth(client, choice);
  }

  /** Starts a login as `did`: the authorization URL to navigate to (after PAR). */
  async begin(did: string, returnTo: string): Promise<URL> {
    const state = JSON.stringify({ returnTo, intended: did } satisfies AppState);
    const candidates = scopeCandidates();
    for (let i = 0; i < candidates.length; i++) {
      try {
        return await this.client.authorize(did, {
          scope: candidates[i],
          state,
          redirect_uri: this.choice.redirectUri as RedirectUri,
        });
      } catch (e) {
        // AUTH-1: the scope fallback is decided at PAR, before the browser opens.
        if (oauthErrorCode(e) === "invalid_scope" && i + 1 < candidates.length) continue;
        throw e;
      }
    }
    throw new Error("no scope to request");
  }

  /** Finishes a login from the callback parameters (`state` and PKCE checked by the library,
   * `iss` against the server that started it). */
  async finish(params: Record<string, string>): Promise<OAuthLogin> {
    const { session, state } = (await this.client.callback(new URLSearchParams(params), {
      redirect_uri: this.choice.redirectUri as RedirectUri,
    })) as { session: OAuthSession; state?: string | null };
    const info = await session.getTokenInfo(false);
    this.sessions.set(session.did, Promise.resolve(session));
    const app = parseAppState(state);
    return {
      did: session.did,
      pds: info.aud,
      issuer: info.iss,
      returnTo: app.returnTo ?? null,
      intended: app.intended ?? null,
    };
  }

  /** The session of `did` on this browser, or null (AuthBridge `SessionProvider`). */
  async session(did: string): Promise<PdsSession | null> {
    let p = this.sessions.get(did);
    if (!p) {
      p = this.client.restore(did);
      this.sessions.set(did, p);
    }
    let s: OAuthSession;
    try {
      s = await p;
    } catch {
      this.sessions.delete(did);
      return null;
    }
    return oauthSession(did, (path, init) => s.fetchHandler(path, init));
  }

  /** Revokes the session at the authorization server and deletes it (AUTH-11). */
  async revoke(did: string): Promise<void> {
    this.sessions.delete(did);
    await this.client.revoke(did);
  }
}
