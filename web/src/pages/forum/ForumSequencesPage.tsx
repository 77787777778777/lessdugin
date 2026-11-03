// SPDX-License-Identifier: AGPL-3.0-or-later
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ActorName } from "../../components/ActorName.tsx";
import { RecordLink } from "../../components/RecordLink.tsx";
import { ErrorView, Loading } from "../../components/Status.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
import { cleanName } from "../../lib/text.ts";
import ui from "../../styles/ui.module.css";
import styles from "../pages.module.css";
import { forumParams, useForumView } from "./forumContext.tsx";

export function ForumSequencesPage() {
  const engine = useEngine();
  const { forum } = useForumView();
  const q = useQuery({
    queryKey: qk.sequences(forum.uri),
    queryFn: () => engine.sequences(forum.uri),
  });
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorView error={q.error} />;
  return (
    <section className={ui.page} aria-label="Sequences">
      <div className={ui.spread}>
        <h2 className={ui.sectionTitle}>Sequences</h2>
        <Link to="/f/$owner/$slug/seqs/new" params={forumParams(forum.uri)} className={ui.button}>
          New sequence
        </Link>
      </div>
      {q.data.length === 0 && <p className={ui.muted}>No sequences yet.</p>}
      <div className={styles.cardList}>
        {q.data.map((s) => (
          <article key={s.uri} className={styles.forumCard}>
            <h3 className={styles.forumCardTitle}>
              {s.pinned && <span title="Pinned">📌 </span>}
              <RecordLink uri={s.uri}>
                <bdi>{cleanName(s.title, 300)}</bdi>
              </RecordLink>
            </h3>
            <p className={ui.small}>
              by <ActorName actor={s.author} /> · {s.chapters} chapters · {s.posts} posts ·{" "}
              {s.score.score} karma
            </p>
          </article>
        ))}
      </div>
    </section>
  );
}
