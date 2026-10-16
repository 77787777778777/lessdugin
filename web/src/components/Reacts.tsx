// SPDX-License-Identifier: AGPL-3.0-or-later
// Reacts (social.md §9): the tallies under an item, and the `default/1` palette
// (docs/spec/reacts-default-1.json). A react made while text in the item is selected is
// anchored to that quote (lfm-1.md §11 "Writers"). Reacts are public; the first one shows an
// interstitial, and each is held for the undo window.

import { DEFAULT_PALETTE } from "@lessdugin/engine/palette";
import type { ReactTally, TextQuote } from "@lessdugin/engine/types";
import { OPINION_UNDO_WINDOW_MS } from "@lessdugin/engine/types";
import { useNavigate } from "@tanstack/react-router";
import { useEffect, useId, useRef, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { writeStore } from "../engine/writes.ts";
import { cx } from "../lib/cx.ts";
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
  quote,
  onPick,
  onClose,
}: {
  quote: TextQuote | null;
  onPick: (id: string) => void;
  onClose: () => void;
}) {
  const [filter, setFilter] = useState("");
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useEffect(() => {
    ref.current?.querySelector("input")?.focus();
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    const onDown = (e: Event): void => {
      if (ref.current && e.target instanceof Node && !ref.current.contains(e.target)) onClose();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown);
    };
  }, [onClose]);
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
      {quote && (
        <p className={styles.quoting}>
          Reacting to the selected text: <q>{quote.exact}</q>
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

type Pending = Record<string, { delta: number; mine: boolean }>;

export function ReactBar({
  subject,
  reacts,
  getQuote,
  ownItem = false,
  signedIn = true,
}: {
  subject: string;
  reacts: ReactTally[];
  /** The current selection inside the item's body, as a quote (null when none). */
  getQuote: () => TextQuote | null;
  ownItem?: boolean | undefined;
  signedIn?: boolean | undefined;
}) {
  const engine = useEngine();
  const requestAck = usePublicAck();
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [quote, setQuote] = useState<TextQuote | null>(null);
  const [pending, setPending] = useState<Pending>({});
  const awaiting = useRef(new Set<string>());

  useEffect(() => {
    void reacts;
    setPending((p) => {
      const next = { ...p };
      for (const k of awaiting.current) delete next[k];
      awaiting.current.clear();
      return next;
    });
  }, [reacts]);

  const send = async (id: string, q: TextQuote | undefined, remove: boolean): Promise<void> => {
    if (!signedIn) {
      void navigate({ to: "/login" });
      return;
    }
    if (!(await requestAck("react"))) return;
    const key = tallyKey(id, q);
    setPending((p) => ({ ...p, [key]: { delta: remove ? -1 : 1, mine: !remove } }));
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
        stance: "for",
        remove,
        ...(q ? { quote: q } : {}),
      });
      writeStore.track(ticket);
      toasts.show({
        message: remove ? "React removed" : "React added (public)",
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
    return p ? { ...t, for: t.for + p.delta, mine: p.mine ? ("for" as const) : undefined } : t;
  });
  for (const [key, p] of Object.entries(pending)) {
    if (p.delta > 0 && !reacts.some((t) => tallyKey(t.react, t.quote) === key)) {
      const [id] = key.split("\u{1f}");
      const r = DEFAULT_PALETTE.reacts.find((x) => x.id === id);
      if (r)
        shown.push({
          react: r.id,
          label: r.label,
          glyph: r.glyph,
          for: 1,
          against: 0,
          mine: "for",
        });
    }
  }

  return (
    <div className={styles.bar}>
      <ul className={styles.tallies} aria-label="Reacts">
        {shown
          .filter((t) => t.for > 0 || t.against > 0)
          .map((t) => {
            const orphan = t.anchor !== undefined && "orphan" in t.anchor && t.anchor.orphan;
            return (
              <li key={tallyKey(t.react, t.quote)}>
                <button
                  type="button"
                  className={cx(styles.tally, t.mine === "for" && styles.mine)}
                  aria-pressed={t.mine === "for"}
                  disabled={ownItem}
                  title={`${t.label}${t.quote ? ` on “${t.quote.exact}”` : ""}${t.against > 0 ? `; ${t.against} against` : ""}`}
                  onClick={() => void send(t.react, t.quote, t.mine === "for")}
                >
                  <span className={styles.glyph} aria-hidden="true">
                    {t.glyph}
                  </span>
                  <span className="visually-hidden">{t.label}: </span>
                  <span>{t.for}</span>
                  {t.against > 0 && <span className={styles.against}>−{t.against}</span>}
                  {t.quote && (
                    <span className={cx(styles.quote, orphan && styles.orphan)}>
                      {orphan ? "quoted text no longer present" : `“${t.quote.exact}”`}
                    </span>
                  )}
                </button>
              </li>
            );
          })}
      </ul>
      {!ownItem && (
        <div className={styles.paletteAnchor}>
          <button
            type="button"
            className={styles.add}
            aria-expanded={open}
            aria-haspopup="dialog"
            onPointerDown={() => setQuote(getQuote())}
            onClick={() => {
              setQuote((q) => q ?? getQuote());
              setOpen((o) => !o);
            }}
          >
            <span aria-hidden="true">☺+</span>
            <span className="visually-hidden">Add react</span>
          </button>
          {open && (
            <ReactPalette
              quote={quote}
              onClose={() => setOpen(false)}
              onPick={(id) => {
                setOpen(false);
                void send(id, quote ?? undefined, false);
              }}
            />
          )}
        </div>
      )}
    </div>
  );
}
