// SPDX-License-Identifier: AGPL-3.0-or-later
// Local drafts (never published until you publish them).

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { RecordLink } from "../components/RecordLink.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import { cleanName } from "../lib/text.ts";
import { fullDate } from "../lib/time.ts";
import { parseAtUri } from "../lib/uri.ts";
import ui from "../styles/ui.module.css";
import styles from "./pages.module.css";

export function DraftsPage() {
  const engine = useEngine();
  const qc = useQueryClient();
  const q = useQuery({ queryKey: qk.drafts(), queryFn: () => engine.drafts() });
  const remove = (id: string): void => {
    void engine.deleteDraft(id).then(() => qc.invalidateQueries({ queryKey: qk.drafts() }));
  };
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Drafts</h1>
      <p className={ui.muted}>Drafts stay on this device until you publish them.</p>
      {q.isPending && <Loading />}
      {q.isError && <ErrorView error={q.error} />}
      {q.data && q.data.length === 0 && <p>No drafts.</p>}
      <div className={styles.cardList}>
        {q.data?.map((d) => {
          const { draft } = d;
          let title: string;
          let text: string;
          let open: React.ReactNode = null;
          if (draft.kind === "post") {
            title = draft.post.title ? cleanName(draft.post.title, 300) : "Untitled post";
            text = draft.post.body.markdown;
            const f = parseAtUri(draft.post.forum);
            if (f) {
              open = (
                <Link
                  to="/f/$owner/$slug/new"
                  params={{ owner: f.did, slug: f.rkey }}
                  search={{ draft: d.id }}
                >
                  Continue editing
                </Link>
              );
            }
          } else if (draft.kind === "comment") {
            title = draft.comment.parent ? "Reply" : "Comment";
            text = draft.comment.body.markdown;
            open = <RecordLink uri={draft.comment.root}>Open the post</RecordLink>;
          } else {
            title = cleanName(draft.sequence.title, 300) || "Sequence";
            text = draft.sequence.body?.markdown ?? "";
          }
          return (
            <article key={d.id} className={styles.forumCard}>
              <h2 className={styles.discussionTitle}>
                <bdi>{title}</bdi>
              </h2>
              <p className={ui.small}>{text.slice(0, 240)}</p>
              <p className={ui.row}>
                <span className={ui.muted}>Saved {fullDate(d.savedAtMs)}</span>
                {open}
                <button type="button" className={ui.danger} onClick={() => remove(d.id)}>
                  Delete
                </button>
              </p>
            </article>
          );
        })}
      </div>
    </div>
  );
}
