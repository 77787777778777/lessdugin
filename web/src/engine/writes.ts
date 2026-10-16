// SPDX-License-Identifier: AGPL-3.0-or-later
// Write tickets as reported by `write` events (engine-api.md §3.5, §3.8).

import type { WriteState, WriteTicket } from "@lessdugin/engine/types";
import { useSyncExternalStore } from "react";

export type TerminalWriteState = Extract<WriteState, { kind: "done" | "failed" | "undone" }>;

function isTerminal(s: WriteState): s is TerminalWriteState {
  return s.kind === "done" || s.kind === "failed" || s.kind === "undone";
}

export class WriteStore {
  private readonly states = new Map<string, WriteState>();
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

  /** Records a ticket returned by a write call (its events may arrive before or after). */
  track(ticket: WriteTicket): void {
    if (!this.states.has(ticket.id)) this.update(ticket.id, ticket.state);
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
