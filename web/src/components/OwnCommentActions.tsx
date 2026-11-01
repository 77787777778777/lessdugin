// SPDX-License-Identifier: AGPL-3.0-or-later
// Edit, retract and delete for the viewer's own comments (data-model.md §8.3, §8.7). Edits keep
// a public revision; retraction keeps the comment, struck through. An edit rebuilds the draft
// from the record itself, so the parent, the quote and the body's media survive it. Before an
// edit, the author is told when it will suspend a keep or a pin (social.md §4.5; the engine
// says which, `editPreflight`).

import type { CommentNode, MediaRef, TextQuote } from "@lessdugin/engine/types";
import { useEffect, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { SpilledBodySourceError, useCoherentEditSource } from "../engine/editSource.ts";
import { writeStore } from "../engine/writes.ts";
import { suspendsNotice } from "../lib/modActions.ts";
import {
  isSpilledBody,
  markdownOf,
  mediaRefsOf,
  objectField,
  refUri,
  textQuoteOf,
} from "../lib/records.ts";
import ui from "../styles/ui.module.css";
import { Dialog } from "./Dialog.tsx";
import { LazyComposer } from "./LazyComposer.tsx";
import { errorMessage } from "./Status.tsx";
import { toasts } from "./Toast.tsx";

export interface CommentSource {
  markdown: string;
  parent?: string;
  quote?: TextQuote;
  media: MediaRef[];
}

/** The editable parts of a comment record, or null if it has no markdown body: inline, or
 * `source`, the markdown of a body that spilled into a blob (`bodySource`). */
export function commentSourceOf(record: unknown, source?: string): CommentSource | null {
  const body = objectField(record, "body");
  const markdown = markdownOf(body) ?? (isSpilledBody(body) ? source : undefined);
  if (markdown === undefined) return null;
  const out: CommentSource = { markdown, media: mediaRefsOf(body) };
  const parent = refUri(record, "parent");
  if (parent !== undefined) out.parent = parent;
  const quote = textQuoteOf(objectField(record, "quote"));
  if (quote) out.quote = quote;
  return out;
}

export function OwnCommentActions({ node, postUri }: { node: CommentNode; postUri: string }) {
  const engine = useEngine();
  const [editing, setEditing] = useState<CommentSource | null>(null);
  const [opening, setOpening] = useState(false);
  const source = useCoherentEditSource(engine, node.uri, opening && editing === null);
  // the edit suspends the comment's positive assertions (a keep, a pin): said before it is sent
  const [suspends, setSuspends] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const run = async (
    what: string,
    start: () => ReturnType<typeof engine.retract>,
  ): Promise<boolean> => {
    try {
      const ticket = await start();
      writeStore.track(ticket, node.uri);
      const final = await writeStore.wait(ticket.id);
      if (final.kind === "failed") throw final.error;
      if (final.kind === "done") toasts.show({ message: what, tone: "success" });
      return final.kind === "done";
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
      return false;
    }
  };

  const sourceData = source.data;
  useEffect(() => {
    if (!opening || editing !== null || !sourceData) return;
    let live = true;
    const draft = commentSourceOf(sourceData.raw.record, sourceData.source.markdown);
    if (!draft) {
      setOpening(false);
      toasts.show({ message: "The comment's source is not available.", tone: "error" });
      return;
    }
    void engine
      .editPreflight(node.uri)
      .then((notices) => {
        if (!live) return;
        setSuspends(suspendsNotice(notices));
        setEditing(draft);
        setOpening(false);
      })
      .catch(() => {
        if (!live) return;
        setSuspends(null);
        setEditing(draft);
        setOpening(false);
      });
    return () => {
      live = false;
    };
  }, [editing, engine, node.uri, opening, sourceData]);

  return (
    <>
      <button
        type="button"
        className={ui.link}
        disabled={opening && source.isFetching}
        onClick={() => {
          setEditing(null);
          setOpening(true);
        }}
      >
        Edit
      </button>
      {opening && source.isError && (
        <div role="alert" className={ui.fullWidth}>
          <p>
            {errorMessage(
              source.error instanceof SpilledBodySourceError
                ? source.error.sourceError
                : source.error,
            )}
          </p>
          <button type="button" className={ui.link} onClick={source.retry}>
            Try again
          </button>
          <button type="button" className={ui.link} onClick={() => setOpening(false)}>
            Cancel
          </button>
        </div>
      )}
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
          {suspends && (
            <p className={ui.notice} role="note" data-testid="edit-suspends">
              {suspends}
            </p>
          )}
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
                        ...(editing.quote !== undefined ? { quote: editing.quote } : {}),
                        body: { markdown, media: editing.media },
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
