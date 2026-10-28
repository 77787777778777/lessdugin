// Attachments for the composer (docs/plan.md M4: "editor ... attachments"): paste, drop or pick
// files, hand each to the engine's `prepareMedia` - which sniffs, strips metadata, applies the
// orientation and enforces the 413 policy - and show what comes back, including the engine's
// refusal reasons verbatim. A file's name is published only when the author asks for it
// (data-model.md §4: `#media.name` is opt-in, a basename only; threat-model PRIV-4), and a
// file without alt text carries the engine's `missingAlt` warning until the author writes one.

import type { MediaRef } from "@lessdugin/engine/types";
import { type Ref, useCallback, useId, useImperativeHandle, useRef, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import styles from "./Attachments.module.css";
import { errorMessage } from "./Status.tsx";

/** One attachment as the composer sees it: the prepared reference, or why it was refused.
 * `name` is the local file name, shown here; the record carries it only through `ref.name`. */
export type Attachment =
  | { kind: "ready"; ref: MediaRef; name: string }
  | { kind: "refused"; reason: string; name: string };

/** Lets the editor hand pasted or dropped files to the attachment list. */
export interface AttachmentsHandle {
  add(files: File[]): void;
}

export interface AttachmentsProps {
  value: Attachment[];
  onChange: (next: Attachment[]) => void;
  /** The media parts of a post, for the publish call. */
  disabled?: boolean | undefined;
}

/** A body's `media` holds at most 128 items (data-model.md §5.1). */
export const MAX_ATTACHMENTS = 128;

/** What the media pipeline takes (data-model.md §8.8); it sniffs the bytes and refuses the
 * rest with its reason. */
export const ATTACHMENT_ACCEPT = "image/*,video/*,audio/*,application/pdf";

function acceptable(file: File): boolean {
  // an unknown type goes to the engine, which sniffs it
  return file.type === "" || /^(image|video|audio)\//.test(file.type) || file.type === "application/pdf";
}

/** The file name as `#media.name` may carry it, or null when readers would ignore it: a
 * basename of at most 255 bytes with no `/`, `\`, C0 or C1 control, U+2028 or U+2029
 * (data-model.md §4). */
export function publishableName(name: string): string | null {
  if (name === "" || new TextEncoder().encode(name).length > 255) return null;
  // biome-ignore lint/suspicious/noControlCharactersInRegex: the controls are what is refused
  return /[/\\\u0000-\u001f\u007f-\u009f\u2028\u2029]/.test(name) ? null : name;
}

/** Prepares one file through the engine and returns the row to show for it. The file name is
 * not sent: it stays local until the author opts in. */
async function prepare(engine: ReturnType<typeof useEngine>, file: File): Promise<Attachment> {
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const ref = await engine.prepareMedia(bytes, {
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

export function Attachments({
  value,
  onChange,
  disabled,
  ref,
}: AttachmentsProps & { ref?: Ref<AttachmentsHandle> | undefined }) {
  const engine = useEngine();
  const inputId = useId();
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const dragDepth = useRef(0);

  const add = useCallback(
    async (files: File[]) => {
      const taken = files.filter(acceptable);
      if (taken.length === 0) return;
      const room = MAX_ATTACHMENTS - value.filter((a) => a.kind === "ready").length;
      const batch = taken.slice(0, Math.max(0, room));
      if (batch.length === 0) return;
      setBusy(true);
      try {
        // sequential, so the engine is not preparing several blobs at once and the rows appear
        // in the order the files were chosen
        const rows: Attachment[] = [];
        for (const f of batch) rows.push(await prepare(engine, f));
        onChange([...value, ...rows]);
      } finally {
        setBusy(false);
      }
    },
    [engine, onChange, value],
  );
  useImperativeHandle(
    ref,
    () => ({
      add: (files: File[]) => {
        if (!disabled && !busy) void add(files);
      },
    }),
    [add, disabled, busy],
  );

  const ready = value.filter((a) => a.kind === "ready").length;
  return (
    <div className={styles.wrap} data-testid="attachments">
      <section
        // a labelled section is a region in the accessibility tree, and the file input beside it
        // is the keyboard path, so these handlers only add the pointer one
        aria-label="Attach files"
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
          accept={ATTACHMENT_ACCEPT}
          multiple
          disabled={disabled || busy || ready >= MAX_ATTACHMENTS}
          data-testid="attachment-input"
          onChange={(e) => {
            void add(Array.from(e.target.files ?? []));
            e.target.value = "";
          }}
        />
        <label htmlFor={inputId} className={styles.pick}>
          {busy ? "Preparing…" : `Attach files (${ready}/${MAX_ATTACHMENTS})`}
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
                {(a.ref.mime.split("/")[1] ?? a.ref.mime).toUpperCase()}
              </span>
              <ReadyFields
                a={a}
                disabled={disabled}
                onChange={(next) => {
                  const rows = [...value];
                  rows[i] = next;
                  onChange(rows);
                }}
              />
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

/** A prepared file's alt text, its missing-alt warning and the file-name opt-in. */
function ReadyFields({
  a,
  disabled,
  onChange,
}: {
  a: Extract<Attachment, { kind: "ready" }>;
  disabled?: boolean | undefined;
  onChange: (next: Attachment) => void;
}) {
  const altWarning =
    (a.ref.alt ?? "").trim() === ""
      ? a.ref.notices?.find((n) => n.code === "missingAlt")
      : undefined;
  const publishable = publishableName(a.name);
  return (
    <div className={styles.fields}>
      <label className={styles.alt}>
        <span>Alt text</span>
        <input
          type="text"
          value={a.ref.alt ?? ""}
          maxLength={2000}
          disabled={disabled}
          onChange={(e) => onChange({ ...a, ref: { ...a.ref, alt: e.target.value } })}
        />
      </label>
      {altWarning && (
        <span className={styles.warning} data-testid="missing-alt">
          {altWarning.message}
        </span>
      )}
      <label className={styles.optIn}>
        <input
          type="checkbox"
          checked={a.ref.name !== undefined}
          disabled={disabled || publishable === null}
          onChange={(e) => {
            const { name: _old, ...ref } = a.ref;
            onChange({
              ...a,
              ref: e.target.checked && publishable !== null ? { ...ref, name: publishable } : ref,
            });
          }}
        />
        <span>
          {publishable === null
            ? "This file name cannot be published"
            : `Publish the file name “${a.name}” (public, permanent)`}
        </span>
      </label>
    </div>
  );
}
