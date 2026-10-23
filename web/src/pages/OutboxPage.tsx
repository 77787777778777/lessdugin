// SPDX-License-Identifier: AGPL-3.0-or-later
// The outbox: writes this device has not finished sending (robustness review H3). They are
// durable, sent one at a time per account, and survive a reload; this page shows why each one
// waits, retries a failed one and undoes one still in its undo window.

import type { WriteState } from "@lessdugin/engine/types";
import { Link } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { errorMessage } from "../components/Status.tsx";
import { toasts } from "../components/Toast.tsx";
import { useEngine } from "../engine/context.tsx";
import {
  canSendNow,
  isWaiting,
  useUnfinishedWrites,
  waitingText,
  writeStore,
} from "../engine/writes.ts";
import ui from "../styles/ui.module.css";
import styles from "./pages.module.css";

/** What the user sees for a write's state. */
export function stateText(s: WriteState, nowMs: number = Date.now()): string {
  switch (s.kind) {
    case "pending":
      if (s.held) return "Waiting: you can still undo it.";
      if (s.needsLogin) return "Waiting: the session ended; it is sent after you sign in again.";
      if (isWaiting(s, nowMs)) {
        const why = s.error ? ` (last try: ${s.error.message})` : "";
        return `Waiting: ${waitingText(s, nowMs)}${why}.`;
      }
      return "Queued: it is sent next.";
    case "sending":
      return "Sending…";
    case "failed":
      return `Not sent: ${s.error.message}`;
    case "done":
      return "Sent.";
    case "undone":
      return "Undone.";
  }
}

export function OutboxPage() {
  const engine = useEngine();
  const writes = useUnfinishedWrites();
  const [busy, setBusy] = useState<string | null>(null);
  // Writes queued before this page was opened (another tab, an earlier visit).
  useEffect(() => {
    engine.writes().then(
      (t) => writeStore.seed(t),
      () => {},
    );
  }, [engine]);
  const act = async (id: string, what: "retry" | "undo"): Promise<void> => {
    // `retry` is also "send now" for a write that backs off after a failure
    setBusy(id);
    try {
      const t = what === "retry" ? await engine.retryWrite(id) : await engine.undo(id);
      writeStore.update(t.id, t.state);
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
    } finally {
      setBusy(null);
    }
  };
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Outbox</h1>
      <p className={ui.muted}>
        Writes this device has not finished sending. They are kept on this device, sent one at a
        time per account, and survive closing the tab.
      </p>
      {writes.length === 0 && <p>Nothing waiting: every write was sent.</p>}
      <ul className={styles.cardList} aria-label="Unsent writes">
        {writes.map(({ id, state }) => (
          <li key={id} className={styles.forumCard} data-write-id={id}>
            <p className={ui.small}>
              <span className={ui.muted}>Write {id}</span>
            </p>
            <p role={state.kind === "failed" ? "alert" : undefined}>{stateText(state)}</p>
            <p className={ui.row}>
              {state.kind === "failed" && (
                <button
                  type="button"
                  className={ui.button}
                  disabled={busy === id}
                  onClick={() => void act(id, "retry")}
                >
                  Retry
                </button>
              )}
              {canSendNow(state) && (
                <button
                  type="button"
                  className={ui.button}
                  disabled={busy === id}
                  onClick={() => void act(id, "retry")}
                >
                  Send now
                </button>
              )}
              {state.kind === "pending" && state.needsLogin && (
                <Link to="/login" className={ui.button}>
                  Sign in again
                </Link>
              )}
              {state.kind === "pending" && state.held && (
                <button
                  type="button"
                  className={ui.ghost}
                  disabled={busy === id}
                  onClick={() => void act(id, "undo")}
                >
                  Undo
                </button>
              )}
            </p>
          </li>
        ))}
      </ul>
    </div>
  );
}
