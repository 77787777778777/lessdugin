// SPDX-License-Identifier: AGPL-3.0-or-later
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { PostListItem } from "../../components/PostListItem.tsx";
import { RenderedHtml } from "../../components/RenderedHtml.tsx";
import { ErrorView, Loading } from "../../components/Status.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
import { cleanName } from "../../lib/text.ts";
import ui from "../../styles/ui.module.css";
import { useForumView } from "./forumContext.tsx";

const route = getRouteApi("/f/$owner/$slug/t/$tag");

export function TagPage() {
  const engine = useEngine();
  const qc = useQueryClient();
  const { forum } = useForumView();
  const { tag } = route.useParams();
  const q = useQuery({
    queryKey: qk.tag(forum.uri, tag),
    queryFn: () => engine.tag(forum.uri, tag),
  });
  const subs = useQuery({ queryKey: qk.subscriptions(), queryFn: () => engine.subscriptions() });
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorView error={q.error} title="Tag not found" />;
  const subscribed =
    subs.data?.some(
      (s) =>
        s.subscription.kind === "tag" &&
        s.subscription.slug === tag &&
        s.subscription.forum === forum.uri,
    ) ?? false;
  return (
    <section className={ui.page} aria-label={`Tag ${tag}`}>
      <div className={ui.spread}>
        <h2 className={ui.pageTitle}>
          <bdi>{cleanName(q.data.tag.name)}</bdi>
        </h2>
        <button
          type="button"
          className={ui.button}
          aria-pressed={subscribed}
          onClick={() =>
            void engine
              .setSubscription({ kind: "tag", forum: forum.uri, slug: tag }, !subscribed)
              .then(() => qc.invalidateQueries({ queryKey: qk.subscriptions() }))
          }
        >
          {subscribed ? "Unsubscribe" : "Subscribe"}
        </button>
      </div>
      {q.data.tag.description && <p className={ui.muted}>{q.data.tag.description}</p>}
      {q.data.page && (
        <div className={ui.card}>
          <RenderedHtml html={q.data.page.html} />
        </div>
      )}
      <h3 className={ui.sectionTitle}>
        {q.data.tag.postCount} {q.data.tag.postCount === 1 ? "post" : "posts"}
      </h3>
      {q.data.posts.map((p) => (
        <PostListItem key={p.uri} post={p} />
      ))}
    </section>
  );
}
