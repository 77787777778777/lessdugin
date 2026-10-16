// SPDX-License-Identifier: AGPL-3.0-or-later
// Moderation actions for authorities (data-model.md §6.7; social.md §4): hide or keep, ban the
// author, pin, set the tier, lock. Each action is a public record with an optional reason.

import type { ModActionValue, Tier } from "@lessdugin/engine/types";
import { useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { writeStore } from "../engine/writes.ts";
import ui from "../styles/ui.module.css";
import { Dialog } from "./Dialog.tsx";
import styles from "./ModMenu.module.css";
import { usePublicAck } from "./PublicAck.tsx";
import { errorMessage } from "./Status.tsx";
import { toasts } from "./Toast.tsx";

type Choice =
  | "hide"
  | "hideSpam"
  | "keep"
  | "ban"
  | "pin"
  | "unpin"
  | "tierPersonal"
  | "tierFrontpage"
  | "tierCurated"
  | "lock"
  | "unlock";

const LABELS: Record<Choice, string> = {
  hide: "Hide",
  hideSpam: "Hide as spam",
  keep: "Keep (override hides and collapses)",
  ban: "Ban the author from this forum",
  pin: "Pin",
  unpin: "Unpin",
  tierPersonal: "Tier: personal",
  tierFrontpage: "Tier: frontpage",
  tierCurated: "Tier: curated",
  lock: "Lock comments",
  unlock: "Unlock comments",
};

export interface ModSubject {
  kind: "post" | "comment";
  uri: string;
  cid: string;
  author: string;
  pinned: boolean;
  locked?: boolean | undefined;
  tier?: Tier | undefined;
}

function actionFor(
  choice: Choice,
  subject: ModSubject,
): { subject: string; action: ModActionValue } {
  const tier = (t: Tier): { subject: string; action: ModActionValue } => ({
    subject: subject.uri,
    action: { kind: "tier", tier: t },
  });
  const pinScope = subject.kind === "post" ? "forum" : "thread";
  switch (choice) {
    case "hide":
      return {
        subject: subject.uri,
        action: { kind: "hide", hidden: true, spam: false, showReason: true },
      };
    case "hideSpam":
      return {
        subject: subject.uri,
        action: { kind: "hide", hidden: true, spam: true, showReason: true },
      };
    case "keep":
      return {
        subject: subject.uri,
        action: { kind: "hide", hidden: false, spam: false, showReason: true },
      };
    case "ban":
      return { subject: subject.author, action: { kind: "access", level: "banned" } };
    case "pin":
      return { subject: subject.uri, action: { kind: "pin", scope: pinScope, pinned: true } };
    case "unpin":
      return { subject: subject.uri, action: { kind: "pin", scope: pinScope, pinned: false } };
    case "tierPersonal":
      return tier("personal");
    case "tierFrontpage":
      return tier("frontpage");
    case "tierCurated":
      return tier("curated");
    case "lock":
      return { subject: subject.uri, action: { kind: "lock", locked: true } };
    case "unlock":
      return { subject: subject.uri, action: { kind: "lock", locked: false } };
  }
}

export function ModMenu({
  forum,
  subject,
  canCurate,
}: {
  forum: string;
  subject: ModSubject;
  canCurate: boolean;
}) {
  const engine = useEngine();
  const requestAck = usePublicAck();
  const [open, setOpen] = useState(false);
  const [choice, setChoice] = useState<Choice>("hide");
  const [reason, setReason] = useState("");
  const [showReason, setShowReason] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const choices: Choice[] = ["hide", "hideSpam", "keep", "ban", subject.pinned ? "unpin" : "pin"];
  if (subject.kind === "post") {
    choices.push("tierPersonal", "tierFrontpage");
    if (canCurate) choices.push("tierCurated");
    choices.push(subject.locked ? "unlock" : "lock");
  }

  const submit = async (): Promise<void> => {
    if (!(await requestAck("modAction"))) return;
    setBusy(true);
    setError(null);
    try {
      const { subject: target, action } = actionFor(choice, subject);
      if (action.kind === "hide") action.showReason = showReason;
      const ticket = await engine.modAction({
        kind: "assert",
        forum,
        scope: "forum",
        subject: target,
        action,
        ...(reason.trim() !== "" ? { reason: reason.trim() } : {}),
      });
      writeStore.track(ticket);
      setOpen(false);
      setReason("");
      toasts.show({ message: `${LABELS[choice]}: sent (public record)` });
      const final = await writeStore.wait(ticket.id);
      if (final.kind === "failed") toasts.show({ message: final.error.message, tone: "error" });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <button
        type="button"
        className={styles.trigger}
        onClick={() => setOpen(true)}
        aria-haspopup="dialog"
      >
        Moderate
      </button>
      <Dialog open={open} onClose={() => setOpen(false)} title={`Moderate this ${subject.kind}`}>
        <fieldset className={styles.choices}>
          <legend className={ui.label}>Action</legend>
          {choices.map((c) => (
            <label key={c} className={styles.choice}>
              <input
                type="radio"
                name="mod-action"
                value={c}
                checked={choice === c}
                onChange={() => setChoice(c)}
              />
              {LABELS[c]}
            </label>
          ))}
        </fieldset>
        <label className={ui.field}>
          <span className={ui.label}>Reason (public)</span>
          <textarea
            className={ui.input}
            rows={3}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
        </label>
        {(choice === "hide" || choice === "hideSpam") && (
          <label className={styles.choice}>
            <input
              type="checkbox"
              checked={showReason}
              onChange={(e) => setShowReason(e.target.checked)}
            />
            Show the reason on the tombstone
          </label>
        )}
        <p className={ui.hint}>
          Moderation actions are records in your repository; they appear in the mod log with their
          reason.
        </p>
        {error && (
          <p className={ui.error} role="alert">
            {error}
          </p>
        )}
        <div className={ui.row}>
          <button
            type="button"
            className={ui.primary}
            disabled={busy}
            onClick={() => void submit()}
          >
            Apply
          </button>
          <button type="button" className={ui.ghost} onClick={() => setOpen(false)}>
            Cancel
          </button>
        </div>
      </Dialog>
    </>
  );
}
