// SPDX-License-Identifier: AGPL-3.0-or-later
// Reacts (social.md §9): the tallies under an item, and the `default/1` palette
// (docs/spec/reacts-default-1.json). A react made while text in the item is selected is
// anchored to that quote (lfm-1.md §11 "Writers"); a selection that cannot be anchored is
// reported in the palette and the react then applies to the whole item. Each tally shows its
// creator, how many seconded it and how many oppose it (§9.2 "Display"), and offers both stances.
// Reacts are public; the first one shows an interstitial, and each is held for the undo window.

import { DEFAULT_PALETTE } from "@lessdugin/engine/palette";
import type { ActorView, ReactTally, Stance, TextQuote } from "@lessdugin/engine/types";
import { isOrphanAnchor, OPINION_UNDO_WINDOW_MS } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { type RefObject, useEffect, useId, useRef, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import { writeStore } from "../engine/writes.ts";
import { cx } from "../lib/cx.ts";
import type { QuoteSelection } from "../lib/quote.ts";
import { cleanName } from "../lib/text.ts";
import { isDid } from "../lib/uri.ts";
import { Badges } from "./Badges.tsx";
import { usePublicAck } from "./PublicAck.tsx";
import styles from "./Reacts.module.css";
import { errorMessage } from "./Status.tsx";
import { toasts } from "./Toast.tsx";

function quoteKey(q: TextQuote | undefined): string {
  return q ? q.exact.normalize("NFC").replace(/\s+/gu, " ").trim() : "";
}

function tallyKey(react: string, q: TextQuote | undefined): string {
  return `${react}\u{1f}${quoteKey(q)}`;
}

export function ReactPalette({
  selection,
  onPick,
  onClose,
  containerRef,
}: {
  selection: QuoteSelection | null;
  onPick: (id: string) => void;
  onClose: () => void;
  /** Clicks inside this element (the palette and its trigger) do not close the palette; the
   * trigger toggles it itself. Defaults to the palette. */
  containerRef?: RefObject<HTMLElement | null> | undefined;
}) {
  const [filter, setFilter] = useState("");
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    ref.current?.querySelector("input")?.focus();
  }, []);
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") close.current();
    };
    const onDown = (e: Event): void => {
      const inside = containerRef?.current ?? ref.current;
      if (inside && e.target instanceof Node && !inside.contains(e.target)) close.current();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown);
    };
  }, [containerRef]);
  const needle = filter.trim().toLowerCase();
  const reacts = DEFAULT_PALETTE.reacts.filter(
    (r) => needle === "" || r.label.toLowerCase().includes(needle) || r.id.includes(needle),
  );
  return (
    <div ref={ref} className={styles.palette} role="dialog" aria-labelledby={titleId}>
      <div className={styles.paletteHead}>
        <h3 id={titleId} className={styles.paletteTitle}>
          Add a react
        </h3>
        <button type="button" className={styles.close} aria-label="Close" onClick={onClose}>
          ×
        </button>
      </div>
      {selection?.kind === "anchored" && (
        <p className={styles.quoting}>
          Reacting to the selected text: <q>{selection.quote.exact}</q>
        </p>
      )}
      {selection?.kind === "unmatched" && (
        <p className={styles.unmatched} role="status">
          The selected text cannot be anchored: it does not appear word for word in the text of this
          item (it may span math, footnote numbers or images). This react will apply to the whole
          item.
        </p>
      )}
      <input
        className={styles.filter}
        type="search"
        placeholder="Filter reacts"
        aria-label="Filter reacts"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
      />
      {DEFAULT_PALETTE.groups.map((g) => {
        const items = reacts.filter((r) => r.group === g.id);
        if (items.length === 0) return null;
        return (
          <section key={g.id} className={styles.group} aria-label={g.label}>
            <h4 className={styles.groupLabel}>{g.label}</h4>
            <div className={styles.grid}>
              {items.map((r) => (
                <button
                  key={r.id}
                  type="button"
                  className={styles.choice}
                  title={r.description}
                  onClick={() => onPick(r.id)}
                >
                  <span className={styles.glyph} aria-hidden="true">
                    {r.glyph}
                  </span>
                  <span>{r.label}</span>
                </button>
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
}

/** A react's creator as a short label. Creators are voters, hence forum participants, so their
 * identities may be resolved eagerly (lfm-1.md §5.4 / F16). */
function useCreatorLabel(did: string | undefined): string | null {
  const engine = useEngine();
  const q = useQuery({
    queryKey: qk.actor(did ?? ""),
    queryFn: () => engine.resolveActor(did ?? ""),
    enabled: did !== undefined && isDid(did),
    staleTime: 5 * 60_000,
  });
  if (!did) return null;
  const actor: ActorView | undefined = q.data;
  if (actor?.handleState === "verified" && actor.handle) return `@${actor.handle}`;
  if (actor?.displayName) return cleanName(actor.displayName);
  return actor?.shortDid ?? did;
}

/** One stance change in flight: the deltas it applies to the shown tally. */
type Pending = Record<string, { forDelta: number; againstDelta: number; mine: Stance | null }>;

function Tally({
  tally,
  ownItem,
  pending,
  onToggle,
}: {
  tally: ReactTally;
  ownItem: boolean;
  pending: boolean;
  onToggle: (stance: Stance) => void;
}) {
  const t = tally;
  const creator = useCreatorLabel(t.creator);
  const orphan = t.anchor !== undefined && isOrphanAnchor(t.anchor);
  const seconded = Math.max(0, t.for - 1);
  const details = [
    creator ? `added by ${creator}` : null,
    seconded > 0 ? `seconded by ${seconded}` : null,
    t.against > 0 ? `${t.against} against` : null,
  ]
    .filter((x) => x !== null)
    .join(", ");
  const quoteText = t.quote ? ` on “${t.quote.exact}”` : "";
  return (
    <li className={styles.tallyItem}>
      <button
        type="button"
        className={cx(styles.tally, t.mine === "for" && styles.mine)}
        aria-pressed={t.mine === "for"}
        disabled={ownItem}
        title={`${t.label}${quoteText}${details ? ` (${details})` : ""}`}
        onClick={() => onToggle("for")}
      >
        <span className={styles.glyph} aria-hidden="true">
          {t.glyph}
        </span>
        <span className="visually-hidden">{t.label}: </span>
        <span>{t.for}</span>
        {t.against > 0 && (
          <span className={styles.against}>
            <span aria-hidden="true">−</span>
            <span className="visually-hidden">, against: </span>
            {t.against}
          </span>
        )}
        {t.quote && (
          <span className={cx(styles.quote, orphan && styles.orphan)}>
            {orphan ? "quoted text no longer present" : `“${t.quote.exact}”`}
          </span>
        )}
        {details && <span className="visually-hidden">; {details}</span>}
      </button>
      {!ownItem && (
        <button
          type="button"
          className={cx(styles.oppose, t.mine === "against" && styles.opposed)}
          aria-pressed={t.mine === "against"}
          aria-label={`Oppose “${t.label}”${quoteText}`}
          title="Oppose this react"
          onClick={() => onToggle("against")}
        >
          <span aria-hidden="true">✗</span>
        </button>
      )}
      {pending && <Badges pending />}
    </li>
  );
}

export function ReactBar({
  subject,
  reacts,
  getQuote,
  ownItem = false,
  signedIn = true,
}: {
  subject: string;
  reacts: ReactTally[];
  /** The current selection inside the item's body (null when there is none). */
  getQuote: () => QuoteSelection | null;
  ownItem?: boolean | undefined;
  signedIn?: boolean | undefined;
}) {
  const engine = useEngine();
  const requestAck = usePublicAck();
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [selection, setSelection] = useState<QuoteSelection | null>(null);
  const [pending, setPending] = useState<Pending>({});
  const awaiting = useRef(new Set<string>());
  const anchorRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    void reacts;
    setPending((p) => {
      const next = { ...p };
      for (const k of awaiting.current) delete next[k];
      awaiting.current.clear();
      return next;
    });
  }, [reacts]);

  /** Sets `stance` for (id, q), or removes the viewer's react when `stance` is null. */
  const send = async (
    id: string,
    q: TextQuote | undefined,
    stance: Stance | null,
    current: Stance | null,
  ): Promise<void> => {
    if (!signedIn) {
      void navigate({ to: "/login" });
      return;
    }
    if (!(await requestAck("react"))) return;
    const key = tallyKey(id, q);
    const forDelta = (stance === "for" ? 1 : 0) - (current === "for" ? 1 : 0);
    const againstDelta = (stance === "against" ? 1 : 0) - (current === "against" ? 1 : 0);
    setPending((p) => ({ ...p, [key]: { forDelta, againstDelta, mine: stance } }));
    const drop = (): void =>
      setPending((p) => {
        const next = { ...p };
        delete next[key];
        return next;
      });
    try {
      const ticket = await engine.opinion(subject, {
        kind: "react",
        id,
        stance: stance ?? current ?? "for",
        remove: stance === null,
        ...(q ? { quote: q } : {}),
      });
      writeStore.track(ticket, subject);
      toasts.show({
        message:
          stance === null
            ? "React removed"
            : stance === "against"
              ? "Opposition recorded (public)"
              : "React added (public)",
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
      if (final.kind === "done") awaiting.current.add(key);
      else drop();
    } catch (e) {
      drop();
      toasts.show({ message: errorMessage(e), tone: "error" });
    }
  };

  const shown = reacts.map((t) => {
    const p = pending[tallyKey(t.react, t.quote)];
    if (!p) return { tally: t, pending: false };
    const tally: ReactTally = {
      ...t,
      for: t.for + p.forDelta,
      against: t.against + p.againstDelta,
    };
    if (p.mine) tally.mine = p.mine;
    else delete tally.mine;
    return { tally, pending: true };
  });
  for (const [key, p] of Object.entries(pending)) {
    if (p.mine !== null && !reacts.some((t) => tallyKey(t.react, t.quote) === key)) {
      const [id, exact] = key.split("\u{1f}");
      const r = DEFAULT_PALETTE.reacts.find((x) => x.id === id);
      if (r) {
        const tally: ReactTally = {
          react: r.id,
          label: r.label,
          glyph: r.glyph,
          for: Math.max(0, p.forDelta),
          against: Math.max(0, p.againstDelta),
          mine: p.mine,
        };
        if (exact) tally.quote = { exact };
        shown.push({ tally, pending: true });
      }
    }
  }

  return (
    <div className={styles.bar}>
      <ul className={styles.tallies} aria-label="Reacts">
        {shown
          .filter(({ tally: t }) => t.for > 0 || t.against > 0)
          .map(({ tally: t, pending: inFlight }) => (
            <Tally
              key={tallyKey(t.react, t.quote)}
              tally={t}
              ownItem={ownItem}
              pending={inFlight}
              onToggle={(stance) => {
                const current = t.mine ?? null;
                void send(t.react, t.quote, current === stance ? null : stance, current);
              }}
            />
          ))}
      </ul>
      {!ownItem && (
        <div className={styles.paletteAnchor} ref={anchorRef}>
          <button
            type="button"
            className={styles.add}
            aria-expanded={open}
            aria-haspopup="dialog"
            onPointerDown={() => {
              if (!open) setSelection(getQuote());
            }}
            onClick={() => {
              if (open) {
                setOpen(false);
                setSelection(null);
                return;
              }
              setSelection((s) => s ?? getQuote());
              setOpen(true);
            }}
          >
            <span aria-hidden="true">☺+</span>
            <span className="visually-hidden">Add react</span>
          </button>
          {open && (
            <ReactPalette
              selection={selection}
              containerRef={anchorRef}
              onClose={() => {
                setOpen(false);
                setSelection(null);
              }}
              onPick={(id) => {
                setOpen(false);
                const q = selection?.kind === "anchored" ? selection.quote : undefined;
                setSelection(null);
                const existing = reacts.find((t) => tallyKey(t.react, t.quote) === tallyKey(id, q));
                void send(id, q, "for", existing?.mine ?? null);
              }}
            />
          )}
        </div>
      )}
    </div>
  );
}
