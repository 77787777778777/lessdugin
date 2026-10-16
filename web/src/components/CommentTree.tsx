// SPDX-License-Identifier: AGPL-3.0-or-later
// A thread as a flat, pre-order list with depth indentation capped at six levels ("continue
// thread →"), collapse/expand, collapsed reasons, tombstones, unread highlights and inline
// reply composers. Long threads are virtualised with @tanstack/react-virtual. Keyboard: `j`/`k`
// move between comments, Enter expands or collapses the focused one.

import type { CommentNode } from "@lessdugin/engine/types";
import { useWindowVirtualizer } from "@tanstack/react-virtual";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { cx } from "../lib/cx.ts";
import { CommentItem, type ThreadContext } from "./CommentItem.tsx";
import styles from "./CommentTree.module.css";
import { RecordLink } from "./RecordLink.tsx";
import { buildRows, type ThreadRow } from "./threadRows.ts";

export const VIRTUALIZE_ABOVE = 80;

function isTypingTarget(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false;
  if (t.isContentEditable || t.closest("[contenteditable='true'], dialog[open]")) return true;
  return ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName) || t.shadowRoot !== null;
}

/** Sets a CSS custom property through the CSSOM (never a style attribute; CSP style-src). */
function useCssVar(ref: React.RefObject<HTMLElement | null>, name: string, value: string): void {
  useLayoutEffect(() => {
    ref.current?.style.setProperty(name, value);
  }, [ref, name, value]);
}

function VirtualRow({
  start,
  index,
  measure,
  children,
}: {
  start: number;
  index: number;
  measure: (el: Element | null) => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useCssVar(ref, "--vt-y", `${start}px`);
  return (
    <div
      ref={(el) => {
        ref.current = el;
        measure(el);
      }}
      className={styles.virtualRow}
      data-index={index}
    >
      {children}
    </div>
  );
}

export function CommentTree({
  comments,
  ctx,
  baseDepth = 1,
  emptyText = "No comments yet.",
}: {
  comments: CommentNode[];
  ctx: ThreadContext;
  baseDepth?: number | undefined;
  emptyText?: string | undefined;
}) {
  const [toggled, setToggled] = useState<ReadonlySet<string>>(() => new Set());
  const [replyTo, setReplyTo] = useState<string | null>(null);
  const [focused, setFocused] = useState<number | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const rows: ThreadRow[] = useMemo(
    () => buildRows(comments, toggled, { baseDepth }),
    [comments, toggled, baseDepth],
  );
  const virtual = rows.length > VIRTUALIZE_ABOVE;

  const [scrollMargin, setScrollMargin] = useState(0);
  useLayoutEffect(() => {
    const el = listRef.current;
    if (el && virtual) setScrollMargin(el.getBoundingClientRect().top + window.scrollY);
  }, [virtual]);

  const virtualizer = useWindowVirtualizer({
    count: virtual ? rows.length : 0,
    estimateSize: (i) => (rows[i]?.kind === "continue" ? 44 : 150),
    overscan: 6,
    scrollMargin,
  });
  useCssVar(listRef, "--vt-total", `${virtual ? virtualizer.getTotalSize() : 0}px`);

  const toggle = useCallback((uri: string) => {
    setToggled((prev) => {
      const next = new Set(prev);
      if (next.has(uri)) next.delete(uri);
      else next.add(uri);
      return next;
    });
  }, []);

  const focusRow = useCallback(
    (index: number) => {
      setFocused(index);
      if (virtual) virtualizer.scrollToIndex(index, { align: "center" });
      let tries = 0;
      const attempt = (): void => {
        const el = listRef.current?.querySelector(`[data-row-index="${index}"]`);
        if (el instanceof HTMLElement) {
          el.focus({ preventScroll: true });
          if (!virtual) el.scrollIntoView?.({ block: "nearest" });
        } else if (tries++ < 10) {
          requestAnimationFrame(attempt);
        }
      };
      requestAnimationFrame(attempt);
    },
    [virtual, virtualizer],
  );

  // j / k navigation between comment rows.
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.defaultPrevented || e.altKey || e.ctrlKey || e.metaKey || isTypingTarget(e.target))
        return;
      if (e.key !== "j" && e.key !== "k") return;
      const step = e.key === "j" ? 1 : -1;
      let i = focused === null ? (step > 0 ? -1 : rows.length) : focused;
      for (;;) {
        i += step;
        if (i < 0 || i >= rows.length) return;
        if (rows[i]?.kind === "comment") break;
      }
      e.preventDefault();
      focusRow(i);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [rows, focused, focusRow]);

  if (rows.length === 0) return <p className={styles.empty}>{emptyText}</p>;

  const renderRow = (row: ThreadRow, index: number): ReactNode => {
    if (row.kind === "continue") {
      return (
        <div className={cx(styles.continue, styles.indent6)}>
          <RecordLink uri={row.parentUri}>
            Continue this thread → ({row.count} more {row.count === 1 ? "reply" : "replies"})
          </RecordLink>
        </div>
      );
    }
    return (
      <CommentItem
        row={row}
        index={index}
        ctx={ctx}
        focused={focused === index}
        replying={replyTo === row.node.uri}
        onToggle={toggle}
        onFocusRow={setFocused}
        onReply={setReplyTo}
      />
    );
  };

  return (
    <div className={styles.treeWrap}>
      <p className={styles.keys}>
        Keyboard: <kbd>j</kbd>/<kbd>k</kbd> next/previous comment, <kbd>Enter</kbd> expand or
        collapse.
      </p>
      {virtual ? (
        <div ref={listRef} className={styles.virtualList} data-virtualized="">
          {virtualizer.getVirtualItems().map((item) => {
            const row = rows[item.index];
            if (!row) return null;
            return (
              <VirtualRow
                key={row.kind === "comment" ? row.node.uri : `continue-${row.parentUri}`}
                start={item.start - scrollMargin}
                index={item.index}
                measure={virtualizer.measureElement}
              >
                {renderRow(row, item.index)}
              </VirtualRow>
            );
          })}
        </div>
      ) : (
        <div ref={listRef} className={styles.list}>
          {rows.map((row, i) => (
            <div key={row.kind === "comment" ? row.node.uri : `continue-${row.parentUri}`}>
              {renderRow(row, i)}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
