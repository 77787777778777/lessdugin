// SPDX-License-Identifier: AGPL-3.0-or-later
// Toasts. They dismiss themselves after `durationMs`, but never while the pointer is over one
// or focus is inside it (WCAG 2.2.1), and when too many are shown the oldest one without an
// action goes first, so an "Undo" is not pushed out by later messages.
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { cx } from "../lib/cx.ts";
import styles from "./Toast.module.css";

export interface ToastOptions {
  message: string;
  tone?: "info" | "success" | "error";
  durationMs?: number;
  action?: { label: string; onAction: () => void };
}

interface ToastItem extends ToastOptions {
  id: number;
}

const MAX_TOASTS = 4;

class ToastStore {
  private items: ToastItem[] = [];
  private nextId = 1;
  private readonly listeners = new Set<() => void>();

  show(opts: ToastOptions): number {
    const id = this.nextId++;
    const items = [...this.items, { ...opts, id }];
    while (items.length > MAX_TOASTS) {
      const plain = items.findIndex((t) => !t.action);
      items.splice(plain >= 0 ? plain : 0, 1);
    }
    this.items = items;
    this.emit();
    return id;
  }

  dismiss(id: number): void {
    const before = this.items.length;
    this.items = this.items.filter((t) => t.id !== id);
    if (this.items.length !== before) this.emit();
  }

  private emit(): void {
    for (const l of [...this.listeners]) l();
  }

  subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };

  snapshot = (): ToastItem[] => this.items;
}

export const toasts = new ToastStore();

function ToastView({ item }: { item: ToastItem }) {
  const { id, durationMs = 5000 } = item;
  const ref = useRef<HTMLDivElement>(null);
  const [paused, setPaused] = useState(false);
  const remaining = useRef(durationMs);
  useEffect(() => {
    if (paused) return;
    const started = Date.now();
    const t = setTimeout(() => toasts.dismiss(id), remaining.current);
    return () => {
      clearTimeout(t);
      remaining.current = Math.max(0, remaining.current - (Date.now() - started));
    };
  }, [paused, id]);
  // Hover or focus inside the toast pauses its timer (listeners on the element: a toast is
  // not itself interactive, its buttons are).
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let hovered = false;
    let focused = false;
    const sync = (): void => setPaused(hovered || focused);
    const enter = (): void => {
      hovered = true;
      sync();
    };
    const leave = (): void => {
      hovered = false;
      sync();
    };
    const focusIn = (): void => {
      focused = true;
      sync();
    };
    const focusOut = (e: FocusEvent): void => {
      if (e.relatedTarget instanceof Node && el.contains(e.relatedTarget)) return;
      focused = false;
      sync();
    };
    el.addEventListener("pointerenter", enter);
    el.addEventListener("pointerleave", leave);
    el.addEventListener("focusin", focusIn);
    el.addEventListener("focusout", focusOut);
    return () => {
      el.removeEventListener("pointerenter", enter);
      el.removeEventListener("pointerleave", leave);
      el.removeEventListener("focusin", focusIn);
      el.removeEventListener("focusout", focusOut);
    };
  }, []);
  return (
    <div
      ref={ref}
      className={cx(
        styles.toast,
        item.tone === "error" && styles.error,
        item.tone === "success" && styles.success,
      )}
      data-paused={paused ? "" : undefined}
    >
      <span className={styles.message}>{item.message}</span>
      {item.action && (
        <button
          type="button"
          className={styles.action}
          onClick={() => {
            item.action?.onAction();
            toasts.dismiss(id);
          }}
        >
          {item.action.label}
        </button>
      )}
      <button
        type="button"
        className={styles.close}
        aria-label="Dismiss"
        onClick={() => toasts.dismiss(id)}
      >
        ×
      </button>
    </div>
  );
}

export function ToastRegion() {
  const items = useSyncExternalStore(toasts.subscribe, toasts.snapshot, toasts.snapshot);
  return (
    <section className={styles.region} aria-live="polite" aria-label="Notifications from the app">
      {items.map((t) => (
        <ToastView key={t.id} item={t} />
      ))}
    </section>
  );
}
