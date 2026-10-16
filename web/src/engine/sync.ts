// SPDX-License-Identifier: AGPL-3.0-or-later
// The host side of sync (engine-api.md §3.3): the app drives the engine's long-lived tasks
// (`liveTail`, `outboxPump`) for as long as it runs, syncs a forum when it is opened and a
// thread when it is read, and shows `syncProgress` events. Results arrive as `changed` events,
// which invalidate queries (App.tsx), so these calls' own results are not used.

import type { EngineApi } from "@lessdugin/engine/api";
import { isEngineError } from "@lessdugin/engine/types";
import { useEffect, useSyncExternalStore } from "react";
import { toasts } from "../components/Toast.tsx";
import { useEngine } from "./context.tsx";

/** Budgets for the syncs a page view starts (the engine may stop early with `partial`). */
export const FORUM_SYNC = { budgetMs: 8_000, maxRequests: 400, full: false } as const;
export const THREAD_SYNC = { budgetMs: 5_000, maxRequests: 100, full: false } as const;

/** An engine without this operation yet (the placeholder wasm adapter) is not an error. */
function report(e: unknown): void {
  if (isEngineError(e) && (e.code === "unsupported" || e.code === "partial")) return;
  toasts.show({
    message: `Sync: ${isEngineError(e) ? e.message : String(e)}`,
    tone: "error",
  });
}

const started = new WeakSet<EngineApi>();

/** Starts the engine's long-lived tasks once per engine instance. */
export function startEngineTasks(engine: EngineApi): void {
  if (started.has(engine)) return;
  started.add(engine);
  engine.liveTail().catch(report);
  engine.outboxPump().catch(report);
  engine.backgroundVerify().catch(report);
}

// ------------------------------------------------------------------------- progress

type Progress = { done: number; total: number };

class SyncProgressStore {
  private readonly byForum = new Map<string, Progress>();
  private readonly listeners = new Set<() => void>();

  update(forum: string, done: number, total: number): void {
    if (total > 0 && done >= total) this.byForum.delete(forum);
    else this.byForum.set(forum, { done, total });
    for (const l of [...this.listeners]) l();
  }

  get(forum: string): Progress | null {
    return this.byForum.get(forum) ?? null;
  }

  subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };
}

export const syncProgress = new SyncProgressStore();

/** The forum's sync progress while a sync is running, or null. */
export function useSyncProgress(forum: string | undefined): Progress | null {
  return useSyncExternalStore(
    syncProgress.subscribe,
    () => (forum ? syncProgress.get(forum) : null),
    () => null,
  );
}

/** Syncs the forum once per visit (the engine schedules authority sync on open, §3.2). */
export function useForumSync(forum: string | undefined): void {
  const engine = useEngine();
  useEffect(() => {
    if (!forum) return;
    engine.syncForum(forum, FORUM_SYNC).catch(report);
  }, [engine, forum]);
}

/** Fetches the thread's participants' ranges once per visit (§3.3 `sync_thread`). */
export function useThreadSync(post: string | undefined): void {
  const engine = useEngine();
  useEffect(() => {
    if (!post) return;
    engine.syncThread(post, THREAD_SYNC).catch(report);
  }, [engine, post]);
}
