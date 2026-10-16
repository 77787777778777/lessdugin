// SPDX-License-Identifier: AGPL-3.0-or-later
import { useEffect, useSyncExternalStore } from "react";
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

class ToastStore {
  private items: ToastItem[] = [];
  private nextId = 1;
  private readonly listeners = new Set<() => void>();

  show(opts: ToastOptions): number {
    const id = this.nextId++;
    this.items = [...this.items.slice(-3), { ...opts, id }];
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
  useEffect(() => {
    const t = setTimeout(() => toasts.dismiss(id), durationMs);
    return () => clearTimeout(t);
  }, [id, durationMs]);
  return (
    <div
      className={cx(
        styles.toast,
        item.tone === "error" && styles.error,
        item.tone === "success" && styles.success,
      )}
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
