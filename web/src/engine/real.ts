// SPDX-License-Identifier: AGPL-3.0-or-later
// The real engine: the Rust core compiled to wasm, running in a worker
// (`@lessdugin/engine/client`, engine-api.md §5), with the host glue of the mode this copy of
// the app is served in (synthesis §6.13):
//
// - hosted (the static site): the OAuth session lives on this page (host/hosted.ts, loaded
//   only when needed); the engine's state is in IndexedDB;
// - `lessdugin web`: the CLI keeps the session and the `state` store; the tab holds a bridge
//   token (host/bridge.ts).
//
// Start-up work that must happen before the hash router mounts happens here, because the app
// renders only after this resolves (main.tsx): the bridge capability exchange, and the OAuth
// callback (threat-model SPA-8), whose outcome decides the first route.
//
// E2E builds (`VITE_E2E=1`) also take the devnet endpoints and the parity time from the test
// harness and install the debug hook (host/e2e.ts); the module is not part of other builds.

import type { EngineApi } from "@lessdugin/engine/api";
import {
  type EngineClient,
  type EngineConfigInput,
  type SessionProvider,
  startEngine,
} from "@lessdugin/engine/client";
import type { ActorView, EngineError } from "@lessdugin/engine/types";
import { isEngineError } from "@lessdugin/engine/types";
import { toasts } from "../components/Toast.tsx";
import { bridgeLogin, bridgeSessions, bridgeToken } from "../host/bridge.ts";
import type { HostedOAuth, HostedOAuthOptions } from "../host/hosted.ts";
import { loginNotice } from "../host/loginNotice.ts";
import { type HostMode, loadHostMode } from "../host/mode.ts";
import { consumeBridgeCapability, consumeOAuthCallback } from "../lib/bootParams.ts";
import { goToSignIn } from "../lib/navigate.ts";
import { withFallbacks } from "./fallbacks.ts";
import type { EngineHandle, LoginProvider } from "./handle.ts";

/** Replaced by a literal at build time: release builds drop every e2e code path. */
const E2E_BUILD: boolean = import.meta.env.VITE_E2E === "1";

function message(e: unknown): string {
  if (isEngineError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Never settles: the page is navigating away (to the authorization server). */
function leaving(): Promise<never> {
  return new Promise(() => {});
}

/** Sets the route the router will mount on (before it mounts). */
function startAt(hash: string): void {
  history.replaceState(history.state, "", `${location.pathname}${location.search}${hash}`);
}

/** The current route, to come back to after a sign-in (the login page itself means home). */
function currentRoute(): string {
  const h = location.hash;
  return h.startsWith("#/") && !h.startsWith("#/login") ? h : "#/";
}

interface Built {
  client: EngineClient;
  handle: EngineHandle;
}

// ------------------------------------------------------------------------------- hosted mode

/** Lazily loads the OAuth client (it is needed only to sign in or to write). */
function lazyOAuth(opts: HostedOAuthOptions): () => Promise<HostedOAuth> {
  let p: Promise<HostedOAuth> | null = null;
  return () => {
    p ??= import("../host/hosted.ts")
      .then((m) => m.HostedOAuth.create(opts))
      .catch((e: unknown) => {
        p = null;
        throw e;
      });
    return p;
  };
}

/** Finishes the OAuth callback the page was opened with: `addAccount` (the login binding),
 * then the first route. Failures are shown on the sign-in page. */
async function finishOAuth(
  params: Record<string, string>,
  api: EngineApi,
  oauth: () => Promise<HostedOAuth>,
): Promise<void> {
  let o: HostedOAuth;
  let login: Awaited<ReturnType<HostedOAuth["finish"]>>;
  try {
    o = await oauth();
    login = await o.finish(params);
  } catch (e) {
    loginNotice.set({ kind: "error", message: `The sign-in did not complete: ${message(e)}` });
    startAt("#/login");
    return;
  }
  const info = {
    did: login.did,
    pds: login.pds,
    auth: "oauth" as const,
    issuer: login.issuer,
    ...(login.intended ? { intended: login.intended } : {}),
  };
  try {
    await api.addAccount(info);
  } catch (e) {
    const detail = isEngineError(e) ? e.detail : undefined;
    if (isEngineError(e) && e.code === "conflict" && detail?.kind === "issuerChanged") {
      // sync.md §3.6: a changed authorization server needs the user's confirmation.
      loginNotice.set({
        kind: "confirmIssuer",
        did: login.did,
        pinned: String(detail.pinned ?? ""),
        issuer: String(detail.issuer ?? login.issuer),
        async confirm() {
          await api.addAccount({ ...info, confirmIssuerChange: true });
          loginNotice.set(null);
        },
        async cancel() {
          await o.revoke(login.did).catch(() => {});
          loginNotice.set(null);
        },
      });
      startAt("#/login");
      return;
    }
    // The login binding failed: the session must not be used (sync.md §3.6).
    await o.revoke(login.did).catch(() => {});
    loginNotice.set({ kind: "error", message: `The sign-in was refused: ${message(e)}` });
    startAt("#/login");
    return;
  }
  toasts.show({ message: "Signed in", tone: "success" });
  startAt(login.returnTo ?? "#/");
}

async function hostedEngine(
  config: EngineConfigInput,
  oauthOpts: HostedOAuthOptions,
): Promise<Built> {
  const oauth = lazyOAuth(oauthOpts);
  const sessions: SessionProvider = {
    session: async (did) => (await oauth()).session(did),
  };
  const client = startEngine({ init: { config }, sessions });
  const api = withFallbacks(client);
  const callback = consumeOAuthCallback();
  if (callback) await finishOAuth(callback as Record<string, string>, api, oauth);
  const login: LoginProvider = {
    description:
      "Sign in with your atproto account. You approve lessdugin on your own server's page; your password never reaches this app.",
    async login(actor: ActorView, opts) {
      const url = await (await oauth()).begin(actor.did, opts?.returnTo ?? currentRoute());
      goToSignIn(url);
      return leaving();
    },
    async logout(did) {
      try {
        await (await oauth()).revoke(did);
      } catch {
        // already gone at the server, or never on this browser: remove it here anyway
      }
      await api.removeAccount(did, false);
    },
  };
  return { client, handle: { kind: "wasm", api, login, version: () => client.version() } };
}

// ------------------------------------------------------------------------- lessdugin web

async function bridgeEngine(config: EngineConfigInput): Promise<Built> {
  const token = await bridgeToken(consumeBridgeCapability());
  const client = startEngine({
    init: { config, state: { kind: "bridge", endpoint: "/bridge/kv", token } },
    sessions: bridgeSessions(token),
  });
  const api = withFallbacks(client);
  const login: LoginProvider = {
    description:
      "Sign in through the lessdugin command-line client that serves this page: it opens your server's sign-in page and keeps the session. This tab never holds it.",
    async login(actor: ActorView) {
      const url = await bridgeLogin(token, actor.did);
      goToSignIn(url);
      return leaving();
    },
    async logout() {
      const e: EngineError = {
        code: "unsupported",
        message:
          "In lessdugin web the session belongs to the command-line client: run `lessdugin logout` in a terminal.",
        retryable: false,
      };
      throw e;
    },
  };
  return {
    client,
    handle: {
      kind: "wasm",
      api,
      login,
      notice:
        "Local mode (lessdugin web): your sign-in stays in the lessdugin command-line client.",
      version: () => client.version(),
    },
  };
}

// --------------------------------------------------------------------------------- entry

export interface RealEngineOptions {
  /** Default: `lessdugin-config.json` (host/mode.ts). */
  mode?: HostMode;
}

export async function createRealEngine(opts: RealEngineOptions = {}): Promise<EngineHandle> {
  const e2e = E2E_BUILD ? await import("../host/e2e.ts") : null;
  const e2eOpts = e2e ? e2e.e2eOptions() : {};
  const config: EngineConfigInput = e2e ? e2e.e2eEngineConfig(e2eOpts) : {};
  const mode = opts.mode ?? (await loadHostMode());
  const built =
    mode === "bridge"
      ? await bridgeEngine(config)
      : await hostedEngine(config, e2eOpts.devnet ? { devnet: { plc: e2eOpts.devnet.plc } } : {});
  if (!e2e) return built.handle;
  e2e.installDebugHook(built.client, e2eOpts);
  const what = [
    e2eOpts.devnet ? `devnet ${e2eOpts.devnet.pds}` : "production network",
    e2eOpts.parityNowMs !== undefined ? `parity time ${e2eOpts.parityNowMs}` : null,
  ]
    .filter(Boolean)
    .join(", ");
  const notice = `End-to-end test build (${what}).${built.handle.notice ? ` ${built.handle.notice}` : ""}`;
  return { ...built.handle, notice };
}
