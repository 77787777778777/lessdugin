// SPDX-License-Identifier: AGPL-3.0-or-later
// Stand-ins for local-state and list operations the wasm engine does not implement yet (they
// reject with `unsupported`, crates/wasm/src/ops.rs `UNSUPPORTED`), so the app shell and the
// M2 pages (forum, post, thread, composer) do not show error boxes for features of later
// milestones. Each stand-in answers only after the engine itself said `unsupported`, so an
// engine that implements the operation is used as soon as it does.
//
// The answers are the truthful ones for this engine: it cannot save drafts, bookmarks or
// subscriptions, compute notifications, tags or sequences, or discover forums, so those lists
// are empty; read marks are not kept. Settings live in this tab's memory until the engine
// stores them (the public-action acknowledgements are then asked again after a reload).

import type { EngineApi } from "@lessdugin/engine/api";
import type { SettingKey, Settings } from "@lessdugin/engine/types";
import { isEngineError } from "@lessdugin/engine/types";

export const DEFAULT_SETTINGS: Settings = Object.freeze({
  sources: "assisted",
  verify: "pds",
  indexers: [],
  externalImagesForums: [],
  mediaAutoloadHosts: [],
  karmaNotifications: "daily",
  acknowledgedPublicActions: [],
}) as Settings;

function clone<T>(v: T): T {
  return structuredClone(v);
}

type Fallbacks = { [K in keyof EngineApi]?: EngineApi[K] };

function stateFallbacks(): Fallbacks {
  let settings: Settings = clone(DEFAULT_SETTINGS);
  return {
    drafts: async () => [],
    bookmarks: async () => [],
    subscriptions: async () => [],
    notifications: async () => ({ dto: 1, items: [] }),
    tags: async () => [],
    sequences: async () => [],
    forumDiscover: async () => [],
    markRead: async () => {},
    settings: async () => clone(settings),
    setSetting: async <K extends SettingKey>(key: K, value: Settings[K]) => {
      settings = { ...settings, [key]: clone(value) };
    },
  };
}

function unsupported(e: unknown): boolean {
  return isEngineError(e) && e.code === "unsupported";
}

/** `api` with the stand-ins above for the operations it rejects as `unsupported`. */
export function withFallbacks<T extends EngineApi>(api: T): T {
  const fallbacks = stateFallbacks() as Record<string, (...args: unknown[]) => Promise<unknown>>;
  return new Proxy(api, {
    get(target, prop, receiver) {
      const value = Reflect.get(target, prop, receiver) as unknown;
      if (typeof prop !== "string") return value;
      const fallback = fallbacks[prop];
      if (!fallback || typeof value !== "function") return value;
      return async (...args: unknown[]) => {
        try {
          return await (value as (...a: unknown[]) => Promise<unknown>).apply(target, args);
        } catch (e) {
          if (unsupported(e)) return fallback(...args);
          throw e;
        }
      };
    },
  });
}
