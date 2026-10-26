// Attachments for the composer (docs/plan.md M4: "editor ... attachments"): paste, drop or pick
// files, hand each to the engine's `prepareMedia` - which sniffs, strips metadata, applies the
// orientation and enforces the 413 policy - and show what comes back, including the engine's
// refusal reasons verbatim.

import type { MediaRef } from "@lessdugin/engine/types";
import { useCallback, useId, useRef, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import styles from "./Attachments.module.css";
import { errorMessage } from "./Status.tsx";

/** One attachment as the composer sees it: the prepared reference, or why it was refused. */
export type Attachment =
  | { kind: "ready"; ref: MediaRef; name: string }
  | { kind: "refused"; reason: string; name: string };

export interface AttachmentsProps {
  value: Attachment[];
  onChange: (next: Attachment[]) => void;
  /** The media parts of a post, for the publish call. */
  disabled?: boolean | undefined;
}

/** Four images per post, which is what a Bluesky post can carry. */
export const MAX_ATTACHMENTS = 4;

/** Prepares one file through the engine and returns the row to show for it. */
async function prepare(engine: ReturnType<typeof useEngine>, file: File): Promise<Attachment> {
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const ref = await engine.prepareMedia(bytes, {
      name: file.name,
      ...(file.type ? { declaredMime: file.type } : {}),
      alt: "",
    });
    return { kind: "ready", ref, name: file.name };
  } catch (err) {
    // The engine's message is written for a person ("image is too large: 9.2 MB, the limit is
    // 5 MB"), so it is shown as-is rather than replaced with a generic failure. `errorMessage`
    // unwraps a structured engine error; a bare `instanceof Error` check turns it into
    // "[object Object]", which is exactly what the first run of this component did.
    return { kind: "refused", reason: errorMessage(err), name: file.name };
  }
}

export function Attachments({ value, onChange, disabled }: AttachmentsProps) {
  const engine = useEngine();
  const inputId = useId();
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const dragDepth = useRef(0);

  const add = useCallback(
    async (files: File[]) => {
      const images = files.filter((f) => f.type.startsWith("image/"));
      if (images.length === 0) return;
      const room = MAX_ATTACHMENTS - value.filter((a) => a.kind === "ready").length;
      const batch = images.slice(0, Math.max(0, room));
      if (batch.length === 0) return;
      setBusy(true);
      try {
        // sequential, so the engine is not preparing four blobs at once and the rows appear in
        // the order the files were chosen
        const rows: Attachment[] = [];
        for (const f of batch) rows.push(await prepare(engine, f));
        onChange([...value, ...rows]);
      } finally {
        setBusy(false);
      }
    },
    [engine, onChange, value],
  );

  const ready = value.filter((a) => a.kind === "ready").length;
  return (
    <div className={styles.wrap} data-testid="attachments">
      <section
        // a labelled section is a region in the accessibility tree, and the file input beside it
        // is the keyboard path, so these handlers only add the pointer one
        aria-label="Attach images"
        className={dragging ? `${styles.zone} ${styles.dragging}` : styles.zone}
        onDragEnter={(e) => {
          if (disabled) return;
          e.preventDefault();
          dragDepth.current += 1;
          setDragging(true);
        }}
        onDragOver={(e) => {
          if (!disabled) e.preventDefault();
        }}
        onDragLeave={(e) => {
          e.preventDefault();
          dragDepth.current = Math.max(0, dragDepth.current - 1);
          if (dragDepth.current === 0) setDragging(false);
        }}
        onDrop={(e) => {
          e.preventDefault();
          dragDepth.current = 0;
          setDragging(false);
          if (disabled) return;
          void add(Array.from(e.dataTransfer.files));
        }}
      >
        <input
          id={inputId}
          className={styles.file}
          type="file"
          accept="image/*"
          multiple
          disabled={disabled || busy || ready >= MAX_ATTACHMENTS}
          data-testid="attachment-input"
          onChange={(e) => {
            void add(Array.from(e.target.files ?? []));
            e.target.value = "";
          }}
        />
        <label htmlFor={inputId} className={styles.pick}>
          {busy ? "Preparing…" : `Attach images (${ready}/${MAX_ATTACHMENTS})`}
        </label>
      </section>
      <AttachmentList value={value} onChange={onChange} disabled={disabled} />
    </div>
  );
}

/** The rows under the picker: what was prepared, and what was refused and why. */
function AttachmentList({ value, onChange, disabled }: AttachmentsProps) {
  if (value.length === 0) return null;
  return (
    <ul className={styles.list} data-testid="attachment-list">
      {value.map((a, i) => (
        <li
          key={a.kind === "ready" ? `${a.name}-${a.ref.cid}` : `${a.name}-refused`}
          className={styles.row}
        >
          {a.kind === "ready" ? (
            <>
              <span className={styles.thumb} aria-hidden="true">
                {a.ref.mime.replace("image/", "").toUpperCase()}
              </span>
              <label className={styles.alt}>
                <span>Alt text</span>
                <input
                  type="text"
                  value={a.ref.alt ?? ""}
                  maxLength={2000}
                  disabled={disabled}
                  onChange={(e) => {
                    const next = [...value];
                    next[i] = { ...a, ref: { ...a.ref, alt: e.target.value } };
                    onChange(next);
                  }}
                />
              </label>
              <span className={styles.ok}>ready</span>
            </>
          ) : (
            <span className={styles.refused} role="alert">
              {a.name}: {a.reason}
            </span>
          )}
          <button
            type="button"
            className={styles.remove}
            disabled={disabled}
            aria-label={`Remove ${a.name}`}
            onClick={() => onChange(value.filter((_, j) => j !== i))}
          >
            ×
          </button>
        </li>
      ))}
    </ul>
  );
}
