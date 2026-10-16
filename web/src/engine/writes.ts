// SPDX-License-Identifier: AGPL-3.0-or-later
// Write tickets as reported by `write` events (engine-api.md §3.5, §3.8), and which subject each
// one is about, for the `[pending]` badge (sync.md §1.6: own item or own opinion with an outbox
// write that is not `done`).

import type { WriteState, WriteTicket } from "@lessdugin/engine/types";
import { useSyncExternalStore } from "react";

export type TerminalWriteState = Extract<WriteState, { kind: "done" | "failed" | "undone" }>;

function isTerminal(s: WriteState): s is TerminalWriteState {
  return s.kind === "done" || s.kind === "failed" || s.kind === "undone";
}

export class WriteStore {
  private readonly states = new Map<string, WriteState>();
  private readonly subjects = new Map<string, string>();
  private readonly listeners = new Set<() => void>();
  private readonly waiters = new Map<string, ((s: TerminalWriteState) => void)[]>();
  private version = 0;

  update(id: string, state: WriteState): void {
    this.states.set(id, state);
    this.version++;
    for (const l of [...this.listeners]) l();
    if (isTerminal(state)) {
      const ws = this.waiters.get(id) ?? [];
      this.waiters.delete(id);
      for (const w of ws) w(state);
    }
  }

  /** Records a ticket returned by a write call (its events may arrive before or after), and the
   * at-uri it writes about (the item edited, voted on, reacted to, …), if any. */
  track(ticket: WriteTicket, subject?: string): void {
    if (subject !== undefined) this.subjects.set(ticket.id, subject);
    if (!this.states.has(ticket.id)) this.update(ticket.id, ticket.state);
    else if (subject !== undefined) {
      this.version++;
      for (const l of [...this.listeners]) l();
    }
  }

  get(id: string): WriteState | undefined {
    return this.states.get(id);
  }

  /** Resolves when the write reaches `done`, `failed` or `undone`. */
  wait(id: string): Promise<TerminalWriteState> {
    const s = this.states.get(id);
    if (s && isTerminal(s)) return Promise.resolve(s);
    return new Promise((resolve) => {
      const ws = this.waiters.get(id) ?? [];
      ws.push(resolve);
      this.waiters.set(id, ws);
    });
  }

  inFlight(): number {
    let n = 0;
    for (const s of this.states.values()) if (s.kind === "pending" || s.kind === "sending") n++;
    return n;
  }

  /** Whether a write about `subject` is pending or being sent. */
  pendingFor(subject: string): boolean {
    for (const [id, s] of this.subjects) {
      if (s !== subject) continue;
      const state = this.states.get(id);
      if (state && !isTerminal(state)) return true;
    }
    return false;
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  snapshot = (): number => this.version;
}

export const writeStore = new WriteStore();

/** The number of writes that are pending or being sent. */
export function useWritesInFlight(store: WriteStore = writeStore): number {
  useSyncExternalStore(store.subscribe, store.snapshot, store.snapshot);
  return store.inFlight();
}

/** Whether an own write about `subject` is not yet done (the `[pending]` badge). */
export function usePendingWrite(subject: string, store: WriteStore = writeStore): boolean {
  useSyncExternalStore(store.subscribe, store.snapshot, store.snapshot);
  return store.pendingFor(subject);
}
