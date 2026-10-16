// SPDX-License-Identifier: AGPL-3.0-or-later
// Placeholder adapter for the real (wasm) engine. Today `@lessdugin/engine/client` exposes only
// `version()`, so every `EngineApi` operation rejects with a clear `unsupported` error until
// lessdugin-wasm implements it (engine-api.md §5).

import type { EngineApi } from "@lessdugin/engine/api";
import { type EngineClient, startEngine } from "@lessdugin/engine/client";
import type { EngineError } from "@lessdugin/engine/types";
import type { EngineHandle } from "./handle.ts";
import { mockAllowed } from "./select.ts";

export function engineNotAvailable(operation: string): EngineError {
  const demo = mockAllowed(import.meta.env)
    ? " Open the app with ?mock to explore it with demo data."
    : "";
  return {
    code: "unsupported",
    message: `engine not yet available: "${operation}" is not implemented by the wasm engine in this build.${demo}`,
    retryable: false,
  };
}

export function createRealEngine(): EngineHandle {
  let client: EngineClient | null = null;
  const api = new Proxy({} as EngineApi, {
    get(_target, prop) {
      if (typeof prop !== "string" || prop === "then") return undefined;
      if (prop === "subscribe") return () => () => {};
      return async () => {
        throw engineNotAvailable(prop);
      };
    },
  });
  return {
    kind: "wasm",
    api,
    login: {
      description: "Sign in with your atproto account (OAuth). Not available in this build yet.",
      async login() {
        throw engineNotAvailable("login");
      },
    },
    version() {
      client ??= startEngine();
      return client.version();
    },
  };
}
