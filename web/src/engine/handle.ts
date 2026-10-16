// SPDX-License-Identifier: AGPL-3.0-or-later
import type { EngineApi } from "@lessdugin/engine/api";
import type { ActorView } from "@lessdugin/engine/types";

/** Host-specific login (engine-api.md §3.1: the host logs in, then calls `addAccount`). */
export interface LoginProvider {
  /** A short description shown on the login page. */
  readonly description: string;
  login(actor: ActorView): Promise<void>;
}

/** The engine the app runs against, plus host glue that is not part of `EngineApi`. */
export interface EngineHandle {
  readonly kind: "mock" | "wasm";
  readonly api: EngineApi;
  readonly login: LoginProvider;
  /** The engine build's version string. */
  version(): Promise<string>;
}
