// SPDX-License-Identifier: AGPL-3.0-or-later
import type { EngineApi } from "@lessdugin/engine/api";
import type { ActorView, Did } from "@lessdugin/engine/types";

/** Host-specific login (engine-api.md §3.1: the host logs in, then calls `addAccount`). */
export interface LoginProvider {
  /** A short description shown on the login page. */
  readonly description: string;
  /** Signs in as `actor`. Resolves once signed in, or never when the page navigates to the
   * authorization server (the result then arrives with the callback). `returnTo` is the
   * `#/…` route to come back to. */
  login(actor: ActorView, opts?: { returnTo?: string }): Promise<void>;
  /** Signs `did` out on this device (revoking the session where the host holds it). */
  logout(did: Did): Promise<void>;
}

/** The engine the app runs against, plus host glue that is not part of `EngineApi`. */
export interface EngineHandle {
  readonly kind: "mock" | "wasm";
  readonly api: EngineApi;
  readonly login: LoginProvider;
  /** A line shown above every page (demo, local or test builds), if any. */
  readonly notice?: string | undefined;
  /** The engine build's version string. */
  version(): Promise<string>;
}
