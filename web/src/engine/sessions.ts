// SPDX-License-Identifier: AGPL-3.0-or-later
// Accounts whose session ended on this device (robustness review H1): the OAuth library dropped
// it (hosted mode), or the engine reported `auth { expired }`. The account stays (its outbox
// waits for a sign-in); the shell shows "sign in again" for it until a sign-in succeeds.

import { useSyncExternalStore } from "react";
import { toasts } from "../components/Toast.tsx";

export const SESSION_ENDED_MESSAGE =
  "Your session has ended. Sign in again: writes you made wait in the outbox until then.";

export class EndedSessions {
  private readonly dids = new Set<string>();
  private readonly listeners = new Set<() => void>();
  private version = 0;

  constructor(private readonly notify: (message: string) => void) {}

  /** `did`'s session ended; the user is told once (until a sign-in clears it). */
  end(did: string): void {
    if (this.dids.has(did)) return;
    this.dids.add(did);
    this.emit();
    this.notify(SESSION_ENDED_MESSAGE);
  }

  /** `did` signed in again (or was removed). */
  clear(did: string): void {
    if (this.dids.delete(did)) this.emit();
  }

  has(did: string): boolean {
    return this.dids.has(did);
  }

  private emit(): void {
    this.version++;
    for (const l of [...this.listeners]) l();
  }

  subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };

  snapshot = (): number => this.version;
}

export const endedSessions = new EndedSessions((message) =>
  toasts.show({ message, tone: "error", durationMs: 15_000 }),
);

/** Whether `did`'s session ended on this device. */
export function useSessionEnded(did: string | undefined, store = endedSessions): boolean {
  useSyncExternalStore(store.subscribe, store.snapshot, store.snapshot);
  return did !== undefined && store.has(did);
}
