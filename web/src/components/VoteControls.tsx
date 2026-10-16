// SPDX-License-Identifier: AGPL-3.0-or-later
// Karma and agreement voting (social.md §7; data-model.md §8.5). Click for a normal vote,
// shift-click or long-press for a strong one; clicking the current vote clears it. Votes are
// applied optimistically, held by the engine for the 4 s undo window (with an "Undo" toast),
// and reconciled when the engine's `changed` event brings the new score. Each vote is tracked
// by its own sequence number: an older vote's completion, failure or undo never touches the
// display of a newer vote on the same axis. The first vote ever shows an interstitial explaining
// that votes are public records. A vote still in the outbox shows `[pending]` (sync.md §1.6).

import type { AgreementValue, KarmaValue, OpinionIntent, ScoreView } from "@lessdugin/engine/types";
import { OPINION_UNDO_WINDOW_MS } from "@lessdugin/engine/types";
import { useNavigate } from "@tanstack/react-router";
import { type MouseEvent, type PointerEvent, useEffect, useId, useRef, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { usePendingWrite, writeStore } from "../engine/writes.ts";
import { cx } from "../lib/cx.ts";
import type { VotingMode } from "../lib/policy.ts";
import { Badges } from "./Badges.tsx";
import { usePublicAck } from "./PublicAck.tsx";
import { errorMessage } from "./Status.tsx";
import { toasts } from "./Toast.tsx";
import styles from "./VoteControls.module.css";

type Axis = "karma" | "agreement";
type AxisValue = KarmaValue | AgreementValue;
type Direction = "up" | "down";

const LONG_PRESS_MS = 500;

/** The UI's estimate of the viewer's vote power, used only until the engine re-scores. */
export function estimatedPower(v: AxisValue | null | undefined): number {
  switch (v) {
    case "up":
    case "agree":
      return 1;
    case "strongUp":
    case "strongAgree":
      return 3;
    case "down":
    case "disagree":
      return -1;
    case "strongDown":
    case "strongDisagree":
      return -3;
    default:
      return 0;
  }
}

function valueFor(axis: Axis, dir: Direction, strong: boolean): AxisValue {
  if (axis === "karma") {
    if (dir === "up") return strong ? "strongUp" : "up";
    return strong ? "strongDown" : "down";
  }
  if (dir === "up") return strong ? "strongAgree" : "agree";
  return strong ? "strongDisagree" : "disagree";
}

function directionOf(v: AxisValue | null | undefined): Direction | null {
  if (v === "up" || v === "strongUp" || v === "agree" || v === "strongAgree") return "up";
  if (v === "down" || v === "strongDown" || v === "disagree" || v === "strongDisagree")
    return "down";
  return null;
}

function isStrong(v: AxisValue | null | undefined): boolean {
  return v === "strongUp" || v === "strongDown" || v === "strongAgree" || v === "strongDisagree";
}

/** Optimistic overrides: a value, or `null` for "cleared"; absent means "use the server's". */
export type Optimistic = { karma?: KarmaValue | null; agreement?: AgreementValue | null };

/** The vote and total shown for one axis, given the server's view and optimistic overrides. */
export function displayedAxis(
  axis: Axis,
  score: ScoreView,
  optimistic: Optimistic,
): { mine: AxisValue | null; total: number } {
  const server = (axis === "karma" ? score.mine?.karma : score.mine?.agreement) ?? null;
  // `agreement` is absent from the wire DTO under `voting = karmaOnly` (engine-api.md §4.1).
  const total = axis === "karma" ? score.score : (score.agreement ?? 0);
  const o = optimistic[axis];
  if (o === undefined) return { mine: server, total };
  return { mine: o, total: total - estimatedPower(server) + estimatedPower(o) };
}

function VoteButton({
  dir,
  axis,
  active,
  strong,
  disabled,
  describedBy,
  onVote,
}: {
  dir: Direction;
  axis: Axis;
  active: boolean;
  strong: boolean;
  disabled: boolean;
  describedBy: string;
  onVote: (strong: boolean) => void;
}) {
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const longPressed = useRef(false);
  const cancel = (): void => clearTimeout(timer.current);
  const label =
    axis === "karma" ? (dir === "up" ? "Upvote" : "Downvote") : dir === "up" ? "Agree" : "Disagree";
  const glyph =
    axis === "karma"
      ? dir === "up"
        ? strong
          ? "⇑"
          : "▲"
        : strong
          ? "⇓"
          : "▼"
      : dir === "up"
        ? "✓"
        : "✗";
  return (
    <button
      type="button"
      className={cx(
        styles.vote,
        active && styles.active,
        active && strong && styles.strong,
        styles[dir],
      )}
      aria-label={`${label}${active ? (strong ? " (strong vote cast)" : " (vote cast)") : ""}`}
      aria-pressed={active}
      aria-describedby={describedBy}
      disabled={disabled}
      onPointerDown={(e: PointerEvent<HTMLButtonElement>) => {
        if (e.button !== 0) return;
        longPressed.current = false;
        cancel();
        timer.current = setTimeout(() => {
          longPressed.current = true;
          onVote(true);
        }, LONG_PRESS_MS);
      }}
      onPointerUp={cancel}
      onPointerLeave={cancel}
      onPointerCancel={cancel}
      onContextMenu={(e) => {
        if (longPressed.current) e.preventDefault();
      }}
      onClick={(e: MouseEvent<HTMLButtonElement>) => {
        cancel();
        if (longPressed.current) {
          longPressed.current = false;
          return;
        }
        onVote(e.shiftKey);
      }}
    >
      <span aria-hidden="true">{glyph}</span>
    </button>
  );
}

export function VoteControls({
  subject,
  score,
  mode = "reacts",
  size = "comment",
  ownItem = false,
  signedIn = true,
}: {
  subject: string;
  score: ScoreView;
  mode?: VotingMode | undefined;
  size?: "post" | "comment" | undefined;
  ownItem?: boolean | undefined;
  signedIn?: boolean | undefined;
}) {
  const engine = useEngine();
  const requestAck = usePublicAck();
  const navigate = useNavigate();
  const hintId = useId();
  const [optimistic, setOptimistic] = useState<Optimistic>({});
  const pending = usePendingWrite(subject);
  /** The sequence number of the vote whose value `optimistic[axis]` shows. */
  const shownSeq = useRef<Record<Axis, number>>({ karma: 0, agreement: 0 });
  const nextSeq = useRef(0);
  /** A completed vote waiting for the engine's re-query to bring its score. */
  const awaitingRefresh = useRef<Record<Axis, number | null>>({ karma: null, agreement: null });

  // A completed write is reconciled when the engine's re-query brings a new score, unless a
  // newer vote on the same axis is still being shown.
  useEffect(() => {
    void score;
    setOptimistic((o) => {
      const next = { ...o };
      for (const axis of ["karma", "agreement"] as const) {
        const seq = awaitingRefresh.current[axis];
        if (seq === null) continue;
        if (shownSeq.current[axis] === seq) delete next[axis];
        awaitingRefresh.current[axis] = null;
      }
      return next;
    });
  }, [score]);

  /** Drops the optimistic value of vote `seq`, if it is still the one shown. */
  const revert = (axis: Axis, seq: number): void =>
    setOptimistic((o) => {
      if (shownSeq.current[axis] !== seq) return o;
      const next = { ...o };
      delete next[axis];
      return next;
    });

  const vote = async (axis: Axis, dir: Direction, strong: boolean): Promise<void> => {
    if (!signedIn) {
      void navigate({ to: "/login" });
      return;
    }
    if (ownItem) return;
    const current = displayedAxis(axis, score, optimistic).mine;
    const target = valueFor(axis, dir, strong);
    const next = current === target ? null : target;
    if (!(await requestAck("vote"))) return;
    nextSeq.current += 1;
    const seq = nextSeq.current;
    shownSeq.current[axis] = seq;
    setOptimistic((o) => ({ ...o, [axis]: next }));
    const intent = (next === null ? { kind: axis } : { kind: axis, value: next }) as OpinionIntent;
    let ticket: Awaited<ReturnType<typeof engine.opinion>>;
    try {
      ticket = await engine.opinion(subject, intent);
    } catch (e) {
      revert(axis, seq);
      toasts.show({ message: errorMessage(e), tone: "error" });
      return;
    }
    writeStore.track(ticket, subject);
    const what = axis === "karma" ? "Vote" : "Agreement vote";
    const toastId = toasts.show({
      message: next === null ? `${what} removed` : `${what} recorded (public)`,
      durationMs: OPINION_UNDO_WINDOW_MS,
      action: {
        label: "Undo",
        onAction: () => {
          engine
            .undo(ticket.id)
            .catch((e: unknown) => toasts.show({ message: errorMessage(e), tone: "error" }));
        },
      },
    });
    const final = await writeStore.wait(ticket.id);
    if (final.kind === "done") {
      awaitingRefresh.current[axis] = seq;
      return;
    }
    revert(axis, seq);
    toasts.dismiss(toastId);
    toasts.show(
      final.kind === "failed"
        ? { message: `${what} failed: ${final.error.message}`, tone: "error" }
        : { message: `${what} undone` },
    );
  };

  const karma = displayedAxis("karma", score, optimistic);
  const agreement = displayedAxis("agreement", score, optimistic);
  const disabled = ownItem;
  const showAgreement = mode !== "karmaOnly";
  return (
    <div className={cx(styles.controls, size === "post" && styles.post)}>
      <span id={hintId} className="visually-hidden">
        {ownItem
          ? "You cannot vote on your own items."
          : "Shift-click or long-press for a strong vote. Votes are public."}
      </span>
      <fieldset className={styles.axis}>
        <legend className="visually-hidden">Karma vote</legend>
        <VoteButton
          axis="karma"
          dir="down"
          active={directionOf(karma.mine) === "down"}
          strong={isStrong(karma.mine)}
          disabled={disabled}
          describedBy={hintId}
          onVote={(s) => void vote("karma", "down", s)}
        />
        <span className={styles.score} title={`${score.unweighted} votes not yet counted`}>
          <span className="visually-hidden">Karma score </span>
          {karma.total}
        </span>
        <VoteButton
          axis="karma"
          dir="up"
          active={directionOf(karma.mine) === "up"}
          strong={isStrong(karma.mine)}
          disabled={disabled}
          describedBy={hintId}
          onVote={(s) => void vote("karma", "up", s)}
        />
      </fieldset>
      {showAgreement && (
        <fieldset className={cx(styles.axis, styles.agreementAxis)}>
          <legend className="visually-hidden">Agreement vote</legend>
          <VoteButton
            axis="agreement"
            dir="down"
            active={directionOf(agreement.mine) === "down"}
            strong={isStrong(agreement.mine)}
            disabled={disabled}
            describedBy={hintId}
            onVote={(s) => void vote("agreement", "down", s)}
          />
          <span className={styles.score}>
            <span className="visually-hidden">Agreement score </span>
            {agreement.total}
          </span>
          <VoteButton
            axis="agreement"
            dir="up"
            active={directionOf(agreement.mine) === "up"}
            strong={isStrong(agreement.mine)}
            disabled={disabled}
            describedBy={hintId}
            onVote={(s) => void vote("agreement", "up", s)}
          />
        </fieldset>
      )}
      {!ownItem && pending && <Badges pending />}
      {size === "post" && score.earlierVersionVotes > 0 && (
        <span className={styles.note}>
          {score.earlierVersionVotes} {score.earlierVersionVotes === 1 ? "vote was" : "votes were"}{" "}
          cast on an earlier version
        </span>
      )}
      {size === "post" && score.unweighted > 0 && (
        <span className={styles.note}>{score.unweighted} votes not yet counted</span>
      )}
    </div>
  );
}
