// SPDX-License-Identifier: AGPL-3.0-or-later
// Telling the user that a write waits in the outbox (robustness review H3): composers let go of
// a write that is waiting (offline, a back-off, a rate limit) or still being sent after
// `QUEUED_AFTER_MS`, clear their text (it is safe in the outbox) and say so, with a way to the
// outbox page, which shows every unfinished write with retry and undo.

import type { WriteState, WriteTicket } from "@lessdugin/engine/types";
import { useNavigate } from "@tanstack/react-router";
import { useCallback } from "react";
import { toasts } from "../components/Toast.tsx";
import { waitingText, writeStore } from "./writes.ts";

/** The message for a queued write (`what`: "Your comment", …). */
export function queuedMessage(what: string, state: WriteState, nowMs = Date.now()): string {
  return `${what} is saved in the outbox: ${waitingText(state, nowMs)}.`;
}

/** Returns `announce(what, state)`, a toast with an "Outbox" action. */
export function useAnnounceQueued(): (what: string, state: WriteState) => void {
  const navigate = useNavigate();
  return useCallback(
    (what: string, state: WriteState) => {
      toasts.show({
        message: queuedMessage(what, state),
        durationMs: 12_000,
        action: { label: "Outbox", onAction: () => void navigate({ to: "/outbox" }) },
      });
    },
    [navigate],
  );
}

/** Returns `send(ticket, what, subject?)` for a form's write: it tracks the write and waits for
 * it (`settle`), resolving with the written uri when it is done and with null when it waits in
 * the outbox (announced as `what`) or was undone; a failed write rejects with its error. */
export function useSendWrite(): (
  ticket: WriteTicket,
  what: string,
  subject?: string,
) => Promise<string | null> {
  const announce = useAnnounceQueued();
  return useCallback(
    async (ticket: WriteTicket, what: string, subject?: string) => {
      writeStore.track(ticket, subject);
      const final = await writeStore.settle(ticket.id);
      if (final.kind === "failed") throw final.error;
      if (final.kind === "queued") {
        announce(what, final.state);
        return null;
      }
      return final.kind === "done" ? final.uri : null;
    },
    [announce],
  );
}
