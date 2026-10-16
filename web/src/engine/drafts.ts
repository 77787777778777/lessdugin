// SPDX-License-Identifier: AGPL-3.0-or-later
// Resuming local comment drafts: a comment composer for (root, parent) starts from the latest
// draft saved for that same place, so the drafts that composers autosave can be continued.

import type { DraftEntry } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { useEngine } from "./context.tsx";
import { qk } from "./queries.ts";

/** The latest saved comment draft for (root, parent), or null. */
export function latestCommentDraft(
  drafts: readonly DraftEntry[],
  root: string,
  parent: string | undefined,
): { id: string; markdown: string } | null {
  let best: DraftEntry | null = null;
  for (const d of drafts) {
    if (d.draft.kind !== "comment") continue;
    const c = d.draft.comment;
    if (c.root !== root || (c.parent ?? null) !== (parent ?? null)) continue;
    if (!best || d.savedAtMs > best.savedAtMs) best = d;
  }
  return best && best.draft.kind === "comment"
    ? { id: best.id, markdown: best.draft.comment.body.markdown }
    : null;
}

/** The resumable draft for a comment composer; `ready` is false until the drafts are loaded
 * (mount the composer only then: it reads its initial text once). */
export function useCommentDraft(
  root: string,
  parent: string | undefined,
  enabled = true,
): { ready: boolean; draft: { id: string; markdown: string } | null } {
  const engine = useEngine();
  const q = useQuery({ queryKey: qk.drafts(), queryFn: () => engine.drafts(), enabled });
  if (!enabled) return { ready: true, draft: null };
  if (q.isPending) return { ready: false, draft: null };
  return { ready: true, draft: q.data ? latestCommentDraft(q.data, root, parent) : null };
}
