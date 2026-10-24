// SPDX-License-Identifier: AGPL-3.0-or-later
// Recent Discussion (social.md §12.5): posts with their newest visible comments.

import { useInfiniteQuery } from "@tanstack/react-query";
import { ActorName } from "../../components/ActorName.tsx";
import { Banners } from "../../components/Banners.tsx";
import { hiddenReasonText } from "../../components/CommentItem.tsx";
import { postTitle } from "../../components/PostMeta.tsx";
import { RecordLink } from "../../components/RecordLink.tsx";
import { RenderedHtml } from "../../components/RenderedHtml.tsx";
import { ErrorView, Loading } from "../../components/Status.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
import { shortAge } from "../../lib/time.ts";
import ui from "../../styles/ui.module.css";
import styles from "../pages.module.css";
import { useForumView } from "./forumContext.tsx";

export function ForumActivityPage() {
  const engine = useEngine();
  const { forum } = useForumView();
  const q = useInfiniteQuery({
    queryKey: qk.discussion(forum.uri),
    queryFn: ({ pageParam }) => engine.recentDiscussion(forum.uri, pageParam, 10),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.cursor,
  });
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorView error={q.error} />;
  const items = q.data.pages.flatMap((p) => p.items);
  return (
    <section className={ui.page} aria-label="Recent discussion">
      <h2 className={ui.sectionTitle}>Recent discussion</h2>
      <Banners banners={q.data.pages[0]?.banners} />
      {items.length === 0 && <p className={ui.muted}>No discussion yet.</p>}
      {items.map(({ post, comments }) => (
        <article key={post.uri} className={styles.discussion}>
          <h3 className={styles.discussionTitle}>
            {post.state.kind === "hidden" || post.state.kind === "tombstone" ? (
              // A hidden post: its reason only, no title or body (social.md §10.1).
              <span className={ui.muted}>[{hiddenReasonText(post.state.reason)}]</span>
            ) : (
              <RecordLink uri={post.uri}>
                <bdi>{postTitle(post)}</bdi>
              </RecordLink>
            )}
          </h3>
          {comments.map((c) => (
            <div key={c.uri} className={styles.miniComment}>
              <div className={styles.miniMeta}>
                <ActorName actor={c.author} />
                <RecordLink uri={c.uri}>{shortAge(c.tRankMs)}</RecordLink>
                <span>{c.score.score} karma</span>
              </div>
              <RenderedHtml html={c.body.html} className="lfm-compact" />
            </div>
          ))}
        </article>
      ))}
      {q.hasNextPage && (
        <button type="button" className={ui.button} onClick={() => void q.fetchNextPage()}>
          Load more
        </button>
      )}
    </section>
  );
}
