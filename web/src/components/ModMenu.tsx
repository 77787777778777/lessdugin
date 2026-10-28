// SPDX-License-Identifier: AGPL-3.0-or-later
// Moderation actions (data-model.md §6.7; social.md §4): for authorities, hide or keep, ban or
// restrict the author (nullifying their votes), pin, set the tier, lock; for a post's author
// (author scope, social.md §4.9), hide or pin a comment in their thread, ban its author from
// their threads, lock their post. Each action is a public record with an optional reason. A ban
// and a restriction are retroactive, which the dialog says before sending (§4.7); the engine's
// notices are shown after. Undoing one's own action removes one's own record (§4.4: writing the
// opposite value does not lift an equal-rank ban, and a keep is not an unhide).

import type {
  ActionTicket,
  Capability,
  ModActionIntent,
  ModActionValue,
  ModScope,
  Tier,
} from "@lessdugin/engine/types";
import { useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { writeStore } from "../engine/writes.ts";
import ui from "../styles/ui.module.css";
import { Dialog } from "./Dialog.tsx";
import styles from "./ModMenu.module.css";
import { usePublicAck } from "./PublicAck.tsx";
import { errorMessage } from "./Status.tsx";
import { toasts } from "./Toast.tsx";

type Assert =
  | "hide"
  | "hideSpam"
  | "keep"
  | "ban"
  | "restrict"
  | "pin"
  | "unpin"
  | "tierPersonal"
  | "tierFrontpage"
  | "tierCurated"
  | "lock"
  | "unlock";

/** The removal of one's own record for an action key (data-model.md §8.4). */
type Remove =
  | "removeHide"
  | "removeAccess"
  | "removeRestrict"
  | "removePin"
  | "removeTier"
  | "removeLock";

type Choice = Assert | Remove;

const LABELS: Record<Choice, string> = {
  hide: "Hide",
  hideSpam: "Hide as spam",
  keep: "Keep (override hides and collapses)",
  ban: "Ban the author from this forum",
  restrict: "Restrict the author",
  pin: "Pin",
  unpin: "Unpin",
  tierPersonal: "Tier: personal",
  tierFrontpage: "Tier: frontpage",
  tierCurated: "Tier: curated",
  lock: "Lock comments",
  unlock: "Unlock comments",
  removeHide: "Remove my hide or keep",
  removeAccess: "Remove my ban of the author (unban)",
  removeRestrict: "Remove my restriction of the author",
  removePin: "Remove my pin",
  removeTier: "Remove my tier",
  removeLock: "Remove my lock",
};

/** Author-scope labels where the forum-scope ones would overstate the effect (§4.9). */
const AUTHOR_LABELS: Partial<Record<Choice, string>> = {
  hide: "Hide from my thread",
  ban: "Ban the author from my threads",
  removeAccess: "Remove my ban of the author from my threads",
};

/** What the dialog says before a retroactive action is sent (social.md §4.6, §4.7). */
export const RETROACTIVE = {
  ban: "A ban is retroactive: it hides all of the author's items in this forum, past ones included, unless they are kept.",
  authorBan:
    "This ban is retroactive: it hides all of the author's comments in your threads, past ones included.",
  restrict:
    "A restriction is retroactive: it also hides the author's earlier items of the restricted kinds.",
} as const;

const CAPABILITIES: { cap: Capability; label: string }[] = [
  { cap: "post", label: "posts" },
  { cap: "comment", label: "comments" },
  { cap: "vote", label: "votes" },
  { cap: "react", label: "reacts" },
  { cap: "tag", label: "tags" },
  { cap: "wiki", label: "wiki edits" },
  { cap: "sequence", label: "sequences" },
];

export interface ModSubject {
  kind: "post" | "comment";
  uri: string;
  cid: string;
  author: string;
  pinned: boolean;
  locked?: boolean | undefined;
  tier?: Tier | undefined;
}

const REMOVE_KEY: Record<Remove, (s: ModSubject) => { subject: string; actionKey: string }> = {
  removeHide: (s) => ({ subject: s.uri, actionKey: "hide" }),
  removeAccess: (s) => ({ subject: s.author, actionKey: "access" }),
  removeRestrict: (s) => ({ subject: s.author, actionKey: "restrict" }),
  removePin: (s) => ({ subject: s.uri, actionKey: s.kind === "post" ? "pin:forum" : "pin:thread" }),
  removeTier: (s) => ({ subject: s.uri, actionKey: "tier" }),
  removeLock: (s) => ({ subject: s.uri, actionKey: "lock" }),
};

const isRemove = (c: Choice): c is Remove => c.startsWith("remove");

function actionFor(
  choice: Assert,
  subject: ModSubject,
  restrict: { capabilities: Capability[]; nullifyVotes: boolean },
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
    case "restrict":
      return {
        subject: subject.author,
        action: {
          kind: "restrict",
          capabilities: restrict.capabilities,
          nullifyVotes: restrict.nullifyVotes,
        },
      };
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

/** The choices for `subject` in `scope` (author scope: social.md §4.9's table only). */
export function modChoices(subject: ModSubject, scope: ModScope, canCurate: boolean): Choice[] {
  if (scope === "author") {
    if (subject.kind === "post") return [subject.locked ? "unlock" : "lock", "removeLock"];
    return [
      "hide",
      "ban",
      subject.pinned ? "unpin" : "pin",
      "removeHide",
      "removeAccess",
      "removePin",
    ];
  }
  const choices: Choice[] = [
    "hide",
    "hideSpam",
    "keep",
    "ban",
    "restrict",
    subject.pinned ? "unpin" : "pin",
  ];
  if (subject.kind === "post") {
    choices.push("tierPersonal", "tierFrontpage");
    if (canCurate) choices.push("tierCurated");
    choices.push(subject.locked ? "unlock" : "lock");
  }
  choices.push("removeHide", "removeAccess", "removeRestrict", "removePin");
  if (subject.kind === "post") choices.push("removeTier", "removeLock");
  return choices;
}

/** Sends a moderation intent after the public-action interstitial, tracks the write and shows
 * the outcome with the engine's notices (the retroactivity of a ban, social.md §4.7). Resolves
 * to whether it was sent; throws the engine's refusal. */
export function useSendModAction(): (intent: ModActionIntent, label: string) => Promise<boolean> {
  const engine = useEngine();
  const requestAck = usePublicAck();
  return async (intent, label) => {
    if (!(await requestAck("modAction"))) return false;
    const ticket: ActionTicket = await engine.modAction(intent);
    writeStore.track(ticket);
    toasts.show({ message: `${label}: sent (public record)` });
    for (const n of ticket.notices ?? []) toasts.show({ message: n.message });
    void writeStore.wait(ticket.id).then((final) => {
      if (final.kind === "failed") toasts.show({ message: final.error.message, tone: "error" });
    });
    return true;
  };
}

export function ModMenu({
  forum,
  subject,
  canCurate,
  scope = "forum",
}: {
  forum: string;
  subject: ModSubject;
  canCurate: boolean;
  /** `author`: the post's author moderating their own thread (social.md §4.9). */
  scope?: ModScope | undefined;
}) {
  const send = useSendModAction();
  const [open, setOpen] = useState(false);
  const [choice, setChoice] = useState<Choice>("hide");
  const [reason, setReason] = useState("");
  const [showReason, setShowReason] = useState(true);
  const [capabilities, setCapabilities] = useState<Capability[]>(["post", "comment"]);
  const [nullifyVotes, setNullifyVotes] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const choices = modChoices(subject, scope, canCurate);
  const label = (c: Choice): string => (scope === "author" && AUTHOR_LABELS[c]) || LABELS[c];
  const retroactive =
    choice === "ban"
      ? scope === "author"
        ? RETROACTIVE.authorBan
        : RETROACTIVE.ban
      : choice === "restrict" && capabilities.length > 0
        ? RETROACTIVE.restrict
        : null;

  const submit = async (): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      let intent: ModActionIntent;
      if (isRemove(choice)) {
        intent = { kind: "remove", forum, scope, ...REMOVE_KEY[choice](subject) };
      } else {
        const { subject: target, action } = actionFor(choice, subject, {
          capabilities,
          nullifyVotes,
        });
        if (action.kind === "hide") action.showReason = showReason;
        intent = {
          kind: "assert",
          forum,
          scope,
          subject: target,
          action,
          ...(reason.trim() !== "" ? { reason: reason.trim() } : {}),
        };
      }
      if (!(await send(intent, label(choice)))) return;
      setOpen(false);
      setReason("");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const radio = (c: Choice) => (
    <label key={c} className={styles.choice}>
      <input
        type="radio"
        name="mod-action"
        value={c}
        checked={choice === c}
        onChange={() => setChoice(c)}
      />
      {label(c)}
    </label>
  );

  return (
    <>
      <button
        type="button"
        className={styles.trigger}
        onClick={() => setOpen(true)}
        aria-haspopup="dialog"
      >
        {scope === "author" ? "Moderate my thread" : "Moderate"}
      </button>
      <Dialog open={open} onClose={() => setOpen(false)} title={`Moderate this ${subject.kind}`}>
        <fieldset className={styles.choices}>
          <legend className={ui.label}>Action</legend>
          {choices.filter((c) => !isRemove(c)).map(radio)}
        </fieldset>
        <fieldset className={styles.choices}>
          <legend className={ui.label}>Undo your own action</legend>
          {choices.filter(isRemove).map(radio)}
        </fieldset>
        {choice === "restrict" && (
          <fieldset className={styles.choices}>
            <legend className={ui.label}>Restricted from</legend>
            {CAPABILITIES.map(({ cap, label: what }) => (
              <label key={cap} className={styles.choice}>
                <input
                  type="checkbox"
                  checked={capabilities.includes(cap)}
                  onChange={(e) =>
                    setCapabilities((cs) =>
                      e.target.checked ? [...cs, cap] : cs.filter((x) => x !== cap),
                    )
                  }
                />
                {what}
              </label>
            ))}
            <label className={styles.choice}>
              <input
                type="checkbox"
                checked={nullifyVotes}
                onChange={(e) => setNullifyVotes(e.target.checked)}
              />
              Nullify their votes (they carry no weight)
            </label>
          </fieldset>
        )}
        {!isRemove(choice) && (
          <label className={ui.field}>
            <span className={ui.label}>Reason (public)</span>
            <textarea
              className={ui.input}
              rows={3}
              value={reason}
              onChange={(e) => setReason(e.target.value)}
            />
          </label>
        )}
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
        {retroactive && (
          <p className={ui.error} role="note" data-testid="mod-retroactive">
            {retroactive}
          </p>
        )}
        <p className={ui.hint}>
          {isRemove(choice)
            ? "This deletes your own record of this action. Another moderator's record stays; to override it, assert the opposite value."
            : "Moderation actions are records in your repository; they appear in the mod log with their reason."}
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
            disabled={busy || (choice === "restrict" && capabilities.length === 0 && !nullifyVotes)}
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
