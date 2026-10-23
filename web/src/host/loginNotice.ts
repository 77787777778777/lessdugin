// SPDX-License-Identifier: AGPL-3.0-or-later
// What the sign-in page shows after a redirect back from the authorization server: the outcome
// of an OAuth callback (handled before the router mounts, threat-model SPA-8) that failed, or
// one that needs the user to confirm a changed authorization server (sync.md §3.6).

import { useSyncExternalStore } from "react";

export type LoginNotice =
  | { kind: "error"; message: string }
  | {
      kind: "confirmIssuer";
      did: string;
      /** The authorization server pinned at the first login. */
      pinned: string;
      /** The one this login used. */
      issuer: string;
      /** Records the account with the new server pinned. */
      confirm(): Promise<void>;
      /** Drops the new session. */
      cancel(): Promise<void>;
    };

let current: LoginNotice | null = null;
const listeners = new Set<() => void>();

export const loginNotice = {
  get(): LoginNotice | null {
    return current;
  },
  set(n: LoginNotice | null): void {
    current = n;
    for (const l of [...listeners]) l();
  },
  subscribe(l: () => void): () => void {
    listeners.add(l);
    return () => {
      listeners.delete(l);
    };
  },
};

export function useLoginNotice(): LoginNotice | null {
  return useSyncExternalStore(loginNotice.subscribe, loginNotice.get, () => null);
}

/** Messages for the engine error codes the `lessdugin web` callback redirects with
 * (`#/login?error=<code>`); the terminal shows the details. */
export function bridgeLoginErrorMessage(code: string): string {
  switch (code) {
    case "conflict":
      return "The sign-in did not match: the account or its authorization server changed. Run `lessdugin login` in the terminal to confirm it.";
    case "auth_required":
      return "The sign-in was cancelled or expired. Try again.";
    case "blocked":
      return "The account's server could not be verified, so the sign-in was refused. The terminal shows why.";
    case "network":
    case "timeout":
      return "The account's server could not be reached. Try again.";
    default:
      return `The sign-in failed (${code.replace(/[^a-z_]/g, "").slice(0, 40)}). The terminal shows why.`;
  }
}
