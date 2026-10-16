// SPDX-License-Identifier: AGPL-3.0-or-later
// Edit, retract and delete for the viewer's own comments (data-model.md §8.3, §8.7). Edits keep
// a public revision; retraction keeps the comment, struck through.

import type { CommentNode } from "@lessdugin/engine/types";
import { useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { writeStore } from "../engine/writes.ts";
import ui from "../styles/ui.module.css";
import { Dialog } from "./Dialog.tsx";
import { LazyComposer } from "./LazyComposer.tsx";
import { errorMessage } from "./Status.tsx";
import { toasts } from "./Toast.tsx";

interface Source {
  markdown: string;
  parent?: string;
}

function sourceOf(record: unknown): Source | null {
  if (typeof record !== "object" || record === null) return null;
  const r = record as { body?: { text?: unknown }; parent?: { uri?: unknown } };
  if (typeof r.body?.text !== "string") return null;
  const out: Source = { markdown: r.body.text };
  if (typeof r.parent?.uri === "string") out.parent = r.parent.uri;
  return out;
}

export function OwnCommentActions({ node, postUri }: { node: CommentNode; postUri: string }) {
  const engine = useEngine();
  const [editing, setEditing] = useState<Source | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const run = async (
    what: string,
    start: () => ReturnType<typeof engine.retract>,
  ): Promise<boolean> => {
    try {
      const ticket = await start();
      writeStore.track(ticket);
      const final = await writeStore.wait(ticket.id);
      if (final.kind === "failed") throw final.error;
      if (final.kind === "done") toasts.show({ message: what, tone: "success" });
      return final.kind === "done";
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
      return false;
    }
  };

  const startEdit = async (): Promise<void> => {
    try {
      const raw = await engine.raw(node.uri);
      const source = sourceOf(raw.record);
      if (!source) throw new Error("The comment's source is not available.");
      setEditing(source);
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
    }
  };

  return (
    <>
      <button type="button" className={ui.link} onClick={() => void startEdit()}>
        Edit
      </button>
      {!node.retracted && (
        <button
          type="button"
          className={ui.link}
          onClick={() => void run("Comment retracted", () => engine.retract(node.uri))}
        >
          Retract
        </button>
      )}
      <button type="button" className={ui.link} onClick={() => setConfirmDelete(true)}>
        Delete
      </button>
      {editing && (
        <div className={ui.fullWidth}>
          <LazyComposer
            compact
            autoFocus
            label="Edit comment"
            initial={editing.markdown}
            submitLabel="Save edit"
            onCancel={() => setEditing(null)}
            onSubmit={async (markdown) => {
              const ok = await run(
                "Comment updated (the previous version stays in its history)",
                () =>
                  engine.edit(
                    node.uri,
                    {
                      kind: "comment",
                      comment: {
                        root: postUri,
                        ...(editing.parent !== undefined ? { parent: editing.parent } : {}),
                        body: { markdown, media: [] },
                      },
                    },
                    { history: true },
                  ),
              );
              if (ok) setEditing(null);
              return ok;
            }}
          />
        </div>
      )}
      <Dialog
        open={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title="Delete this comment?"
      >
        <p>
          The comment and its edit history are deleted from your repository. Replies stay, under a
          "missing comment" placeholder. Copies others have archived may remain.
        </p>
        <div className={ui.row}>
          <button
            type="button"
            className={ui.danger}
            onClick={() => {
              setConfirmDelete(false);
              void run("Comment deleted", () => engine.delete(node.uri));
            }}
          >
            Delete
          </button>
          <button type="button" className={ui.ghost} onClick={() => setConfirmDelete(false)}>
            Cancel
          </button>
        </div>
      </Dialog>
    </>
  );
}
