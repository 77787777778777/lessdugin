// SPDX-License-Identifier: AGPL-3.0-or-later
// Stand-ins for local-state and list operations the wasm engine does not implement yet (they
// reject with `unsupported`, crates/wasm/src/ops.rs `UNSUPPORTED`), so the app shell and the
// M2 pages (forum, post, thread, composer) do not show error boxes for features of later
// milestones. Each stand-in answers only after the engine itself said `unsupported`, so an
// engine that implements the operation is used as soon as it does.
//
// The answers are the truthful ones for this engine: it cannot save bookmarks or
// subscriptions, compute notifications, tags or sequences, or discover forums, so those lists
// are empty; read marks are not kept. Settings live in this tab's memory until the engine
// stores them (the public-action acknowledgements are then asked again after a reload).
//
// Drafts are kept by a `DraftBackend` the host chooses, so that composer text survives a
// reload, a failed send and a reply box scrolled out of a long thread (robustness review H3):
// `localStorage` on the hosted site, this tab's memory under `lessdugin web` (whose browser
// keeps no drafts, threat-model §7; they belong in the native `state.sqlite`).

import type { EngineApi } from "@lessdugin/engine/api";
import type { Draft, DraftEntry, EngineError, SettingKey, Settings } from "@lessdugin/engine/types";
import { isEngineError } from "@lessdugin/engine/types";

/** Where the stand-in drafts live. */
export interface DraftBackend {
  load(): DraftEntry[];
  /** Throws when the entries cannot be kept (storage full). */
  store(entries: DraftEntry[]): void;
}

/** Drafts kept at most (oldest dropped first) and the largest draft text, in UTF-16 units
 * (`limits.md` §11 `DRAFT_MAX_BYTES` is 2 MiB of UTF-8; browser storage is far smaller). */
export const DRAFTS_MAX = 50;
export const DRAFT_TEXT_MAX = 1_000_000;

function isDraftEntry(v: unknown): v is DraftEntry {
  if (typeof v !== "object" || v === null) return false;
  const e = v as Record<string, unknown>;
  const d = e.draft as Record<string, unknown> | null | undefined;
  return (
    typeof e.id === "string" &&
    typeof e.savedAtMs === "number" &&
    typeof d === "object" &&
    d !== null &&
    (d.kind === "post" || d.kind === "comment") &&
    typeof d[d.kind as string] === "object"
  );
}

/** Drafts in this tab's memory only. */
export function memoryDrafts(): DraftBackend {
  let entries: DraftEntry[] = [];
  return {
    load: () => clone(entries),
    store: (e) => {
      entries = clone(e);
    },
  };
}

/** Drafts in `storage` (`localStorage`) under `key`; unreadable content counts as none. */
export function storageDrafts(storage: Storage, key = "lessdugin-drafts-v1"): DraftBackend {
  return {
    load() {
      try {
        const v = JSON.parse(storage.getItem(key) ?? "[]") as unknown;
        return Array.isArray(v) ? v.filter(isDraftEntry) : [];
      } catch {
        return [];
      }
    },
    store(entries) {
      storage.setItem(key, JSON.stringify(entries));
    },
  };
}

/** `localStorage` when this page may use it, else `null`. */
export function pageStorage(): Storage | null {
  try {
    const s = globalThis.localStorage;
    return s ?? null;
  } catch {
    return null;
  }
}

function draftText(d: Draft): string {
  return d.kind === "post" ? d.post.body.markdown : d.comment.body.markdown;
}

function newDraftId(): string {
  const b = new Uint8Array(8);
  crypto.getRandomValues(b);
  return `local-${Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("")}`;
}

function tooLarge(message: string): EngineError {
  return { code: "too_large", message, retryable: false };
}

function draftFallbacks(backend: DraftBackend): Fallbacks {
  return {
    drafts: async () => backend.load().sort((a, b) => b.savedAtMs - a.savedAtMs),
    saveDraft: async (draft: Draft) => {
      if (draftText(draft).length > DRAFT_TEXT_MAX) throw tooLarge("the draft is too long to keep");
      const id = draft.id ?? newDraftId();
      const entry: DraftEntry = { id, savedAtMs: Date.now(), draft: { ...clone(draft), id } };
      const others = backend
        .load()
        .filter((e) => e.id !== id)
        .sort((a, b) => b.savedAtMs - a.savedAtMs)
        .slice(0, DRAFTS_MAX - 1);
      try {
        backend.store([entry, ...others]);
      } catch {
        throw tooLarge("this browser's storage for the site is full: the draft was not saved");
      }
      return id;
    },
    deleteDraft: async (id: string) => {
      const entries = backend.load();
      const rest = entries.filter((e) => e.id !== id);
      if (rest.length !== entries.length) backend.store(rest);
    },
  };
}

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
    bookmarks: async () => [],
    subscriptions: async () => [],
    notifications: async () => ({ dto: 1, items: [], unread: 0 }),
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

export interface FallbackOptions {
  /** Where drafts live while the engine cannot keep them (default: this tab's memory). */
  drafts?: DraftBackend;
}

/** `api` with the stand-ins above for the operations it rejects as `unsupported`. */
export function withFallbacks<T extends EngineApi>(api: T, opts: FallbackOptions = {}): T {
  const fallbacks = {
    ...stateFallbacks(),
    ...draftFallbacks(opts.drafts ?? memoryDrafts()),
  } as Record<string, (...args: unknown[]) => Promise<unknown>>;
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
