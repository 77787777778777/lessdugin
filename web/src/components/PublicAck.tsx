// SPDX-License-Identifier: AGPL-3.0-or-later
// First-use interstitials for public actions (social.md §17; synthesis P8): votes, reacts,
// tag votes, memberships and moderation actions are public records in the user's repository.

import type { PublicActionKind } from "@lessdugin/engine/types";
import { useQueryClient } from "@tanstack/react-query";
import { createContext, type ReactNode, use, useCallback, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import ui from "../styles/ui.module.css";
import { Dialog } from "./Dialog.tsx";

type Request = (kind: PublicActionKind) => Promise<boolean>;

const PublicAckContext = createContext<Request>(async () => true);

const COPY: Record<PublicActionKind, { title: string; body: string[] }> = {
  vote: {
    title: "Votes are public",
    body: [
      "Your votes are records in your own atproto repository, not private ballots. Anyone can see how you voted on what, and archives may keep a vote after you change or remove it.",
      "Each vote is held for 4 seconds before it is sent, so you can undo it.",
    ],
  },
  react: {
    title: "Reacts are public",
    body: [
      "Reacts are stored in your repository next to your votes. Everyone can see which reacts you added, including the text you quoted.",
    ],
  },
  tagVote: {
    title: "Tag votes are public",
    body: ["Tag votes are stored in your repository and anyone can see them."],
  },
  join: {
    title: "Membership is public",
    body: [
      "Joining writes a membership record to your repository. It, and any application note, is visible to everyone.",
    ],
  },
  modAction: {
    title: "Moderation actions are public",
    body: [
      "Moderation actions, with their reasons, are records in your repository. They appear in the forum's mod log and in archives.",
    ],
  },
};

export function PublicAckProvider({ children }: { children: ReactNode }) {
  const engine = useEngine();
  const qc = useQueryClient();
  const [pending, setPending] = useState<{
    kind: PublicActionKind;
    resolve: (ok: boolean) => void;
  } | null>(null);

  const request = useCallback<Request>(
    async (kind) => {
      const settings = await qc.fetchQuery({
        queryKey: qk.settings(),
        queryFn: () => engine.settings(),
      });
      if (settings.acknowledgedPublicActions.includes(kind)) return true;
      return new Promise<boolean>((resolve) => setPending({ kind, resolve }));
    },
    [engine, qc],
  );

  const answer = async (ok: boolean): Promise<void> => {
    const p = pending;
    if (!p) return;
    setPending(null);
    if (ok) {
      const s = await engine.settings();
      await engine.setSetting("acknowledgedPublicActions", [
        ...new Set([...s.acknowledgedPublicActions, p.kind]),
      ]);
      await qc.invalidateQueries({ queryKey: qk.settings() });
    }
    p.resolve(ok);
  };

  const copy = pending ? COPY[pending.kind] : null;
  return (
    <PublicAckContext value={request}>
      {children}
      <Dialog open={pending !== null} onClose={() => void answer(false)} title={copy?.title ?? ""}>
        {copy?.body.map((p) => (
          <p key={p}>{p}</p>
        ))}
        <div className={ui.row}>
          <button type="button" className={ui.primary} onClick={() => void answer(true)}>
            I understand, continue
          </button>
          <button type="button" className={ui.ghost} onClick={() => void answer(false)}>
            Cancel
          </button>
        </div>
      </Dialog>
    </PublicAckContext>
  );
}

/** Asks for the one-time acknowledgement of a public action kind; resolves to false if the
 * user cancels. */
export function usePublicAck(): Request {
  return use(PublicAckContext);
}
