// SPDX-License-Identifier: AGPL-3.0-or-later
// Write tickets as reported by `write` events (engine-api.md §3.5, §3.8), and which subject each
// one is about, for the `[pending]` badge (sync.md §1.6: own item or own opinion with an outbox
// write that is not `done`).

import type { WriteState, WriteTicket } from "@lessdugin/engine/types";
import { useSyncExternalStore } from "react";

export type TerminalWriteState = Extract<WriteState, { kind: "done" | "failed" | "undone" }>;

/** What `settle` resolves with: the final state, or `queued` when the write waits in the
 * outbox (it is durable there and will be sent later). */
export type SettledWrite = TerminalWriteState | { kind: "queued"; state: WriteState };

/** How long a composer waits for a write that is being sent before it lets go (the write
 * carries on in the background). */
export const QUEUED_AFTER_MS = 10_000;

export function isTerminal(s: WriteState): s is TerminalWriteState {
  return s.kind === "done" || s.kind === "failed" || s.kind === "undone";
}

/** A pending write that waits for a back-off, a rate limit, the write budget or a new login
 * of its account (not for the undo window). */
export function isWaiting(s: WriteState, nowMs: number): boolean {
  if (s.kind !== "pending" || s.held === true) return false;
  return (
    s.throttled === true ||
    s.needsLogin === true ||
    (s.notBeforeMs !== undefined && s.notBeforeMs > nowMs)
  );
}

/** A pending write that backs off after a failure, which `retryWrite` sends at once
 * (engine-api.md §3.5; the undo window, the write budget and a rate limit are not shortened). */
export function canSendNow(s: WriteState): boolean {
  return (
    s.kind === "pending" &&
    s.held !== true &&
    s.needsLogin !== true &&
    s.throttled === true &&
    s.error !== undefined &&
    s.error.code !== "rate_limited"
  );
}

/** Only acknowledgements are durable. Report lines and record content are recovered from the
 * engine, never copied into site storage. The injectable shape is localStorage's relevant part. */
export interface ReportAcknowledgements {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

const REPORT_ACK_KEY = "lessdugin.adoptionReports.acknowledged.v1";
const REPORT_ACK_CAP = 256;

function browserReportAcknowledgements(): ReportAcknowledgements | undefined {
  try {
    return typeof window === "undefined" ? undefined : window.localStorage;
  } catch {
    return undefined;
  }
}

/** The report of a done adoption (`WriteState.done.report`, engine-api.md §3.5 `mod_adopt`). */
export interface AdoptionReportEntry {
  id: string;
  lines: string[];
}

export class WriteStore {
  private readonly states = new Map<string, WriteState>();
  private readonly subjects = new Map<string, string>();
  private readonly listeners = new Set<() => void>();
  private readonly waiters = new Map<string, ((s: TerminalWriteState) => void)[]>();
  /** The adoption reports of done writes (a `modAdopt`, also the one `modsSet` runs first), as
   * their `done` events arrive and until shown, each write's once (the TUI shows them on its
   * `Write` event path too; review finding sec-8). */
  private readonly reports: AdoptionReportEntry[] = [];
  private readonly reported = new Set<string>();
  private readonly acknowledged: Set<string>;
  private version = 0;

  constructor(private readonly acknowledgements?: ReportAcknowledgements) {
    let ids: string[] = [];
    try {
      const parsed: unknown = JSON.parse(acknowledgements?.getItem(REPORT_ACK_KEY) ?? "[]");
      if (Array.isArray(parsed)) {
        ids = parsed.filter((id): id is string => typeof id === "string").slice(-REPORT_ACK_CAP);
      }
    } catch {
      // Malformed or unavailable storage degrades to at-least-once display, not lost reports.
    }
    this.acknowledged = new Set(ids);
  }

  private persistAcknowledgements(): void {
    try {
      this.acknowledgements?.setItem(
        REPORT_ACK_KEY,
        JSON.stringify([...this.acknowledged].slice(-REPORT_ACK_CAP)),
      );
    } catch {
      // The in-memory acknowledgement still prevents a replay in this page lifetime.
    }
  }

  /** Queues a report once, however the done state arrived (event, ticket or outbox listing). */
  private record(id: string, state: WriteState): void {
    this.states.set(id, state);
    if (
      state.kind === "done" &&
      state.report !== undefined &&
      state.report.length > 0 &&
      !this.reported.has(id) &&
      !this.acknowledged.has(id)
    ) {
      this.reported.add(id);
      this.reports.push({ id, lines: [...state.report] });
    }
  }

  update(id: string, state: WriteState): void {
    this.record(id, state);
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
    if (!this.states.has(ticket.id)) this.record(ticket.id, ticket.state);
    else if (subject !== undefined) {
      this.version++;
      for (const l of [...this.listeners]) l();
    }
  }

  /** Adds the writes of `writes()` that no event has reported yet (writes queued before this
   * page loaded, or by another tab). */
  seed(tickets: readonly WriteTicket[]): void {
    for (const t of tickets) {
      // A listing can be newer than a ticket/event we already saw, or an older snapshot. Only
      // an existing terminal state is authoritative over it; otherwise reconcile through update
      // so a done report is recorded and its waiters are released exactly as for an event.
      const current = this.states.get(t.id);
      if (
        current !== undefined &&
        (isTerminal(current) || (current.kind === "sending" && t.state.kind === "pending"))
      ) {
        continue;
      }
      this.update(t.id, t.state);
    }
  }

  get(id: string): WriteState | undefined {
    return this.states.get(id);
  }

  /** The oldest adoption report not shown yet for `account`. Real write ids are
   * `<account DID>/<seq>` (crates/core/src/outbox/model.rs `write_id`); this changes no wire
   * field and leaves other accounts' reports queued for their own presentation context. */
  pendingReport(account?: string): AdoptionReportEntry | undefined {
    return this.reports.find((r) => account !== undefined && r.id.startsWith(`${account}/`));
  }

  dismissReport(id: string): void {
    const i = this.reports.findIndex((r) => r.id === id);
    if (i >= 0) this.reports.splice(i, 1);
    this.acknowledged.add(id);
    this.persistAcknowledgements();
    this.version++;
    for (const l of [...this.listeners]) l();
  }

  /** Every known write that is not done or undone, in the order first seen. */
  unfinished(): { id: string; state: WriteState }[] {
    const out: { id: string; state: WriteState }[] = [];
    for (const [id, state] of this.states) {
      if (state.kind !== "done" && state.kind !== "undone") out.push({ id, state });
    }
    return out;
  }

  /** The number of writes that failed (and wait for a retry or to be dropped). */
  failedCount(): number {
    let n = 0;
    for (const s of this.states.values()) if (s.kind === "failed") n++;
    return n;
  }

  /** Resolves when the write is done, failed or undone, or with `queued` as soon as it waits
   * (`throttled`, or a `notBeforeMs` in the future) or is still unsent after `graceMs`. A
   * composer clears on `queued`: the text is safe in the outbox (robustness review H3). */
  settle(
    id: string,
    graceMs = QUEUED_AFTER_MS,
    now: () => number = Date.now,
  ): Promise<SettledWrite> {
    return new Promise((resolve) => {
      let finished = false;
      let unsubscribe = (): void => {};
      let timer: ReturnType<typeof setTimeout> | undefined;
      const finish = (v: SettledWrite): void => {
        if (finished) return;
        finished = true;
        unsubscribe();
        if (timer !== undefined) clearTimeout(timer);
        resolve(v);
      };
      const check = (): void => {
        const s = this.states.get(id);
        if (!s) return;
        if (isTerminal(s)) finish(s);
        else if (isWaiting(s, now())) finish({ kind: "queued", state: s });
      };
      unsubscribe = this.subscribe(check);
      timer = setTimeout(() => {
        const s = this.states.get(id) ?? { kind: "pending" };
        finish(isTerminal(s) ? s : { kind: "queued", state: s });
      }, graceMs);
      check();
    });
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

export const writeStore = new WriteStore(browserReportAcknowledgements());

/** The number of writes that are pending or being sent. */
export function useWritesInFlight(store: WriteStore = writeStore): number {
  useSyncExternalStore(store.subscribe, store.snapshot, store.snapshot);
  return store.inFlight();
}

/** The number of failed writes. */
export function useFailedWrites(store: WriteStore = writeStore): number {
  useSyncExternalStore(store.subscribe, store.snapshot, store.snapshot);
  return store.failedCount();
}

/** The writes that are not done or undone. */
export function useUnfinishedWrites(
  store: WriteStore = writeStore,
): { id: string; state: WriteState }[] {
  useSyncExternalStore(store.subscribe, store.snapshot, store.snapshot);
  return store.unfinished();
}

/** Describes a write that waits, for a "queued" message. */
export function waitingText(s: WriteState, nowMs: number = Date.now()): string {
  if (s.kind === "sending") return "it is still being sent and will finish in the background";
  if (s.kind === "pending" && s.needsLogin === true) {
    return "it will be sent after you sign in to this account again";
  }
  if (s.kind === "pending" && s.notBeforeMs !== undefined && s.notBeforeMs > nowMs) {
    const at = new Date(s.notBeforeMs).toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
    });
    return `it will be sent when the connection allows (next try around ${at})`;
  }
  return "it will be sent when the connection allows";
}

/** Whether an own write about `subject` is not yet done (the `[pending]` badge). */
export function usePendingWrite(subject: string, store: WriteStore = writeStore): boolean {
  useSyncExternalStore(store.subscribe, store.snapshot, store.snapshot);
  return store.pendingFor(subject);
}
