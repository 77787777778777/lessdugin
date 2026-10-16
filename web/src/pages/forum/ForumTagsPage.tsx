// SPDX-License-Identifier: AGPL-3.0-or-later
import { useQuery } from "@tanstack/react-query";
import { getRouteApi, Link } from "@tanstack/react-router";
import { ErrorView, Loading } from "../../components/Status.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
import { cleanName } from "../../lib/text.ts";
import ui from "../../styles/ui.module.css";
import { useForumView } from "./forumContext.tsx";

const route = getRouteApi("/f/$owner/$slug/tags");

export function ForumTagsPage() {
  const engine = useEngine();
  const { forum } = useForumView();
  const params = route.useParams();
  const tags = useQuery({ queryKey: qk.tags(forum.uri), queryFn: () => engine.tags(forum.uri) });
  if (tags.isPending) return <Loading />;
  if (tags.isError) return <ErrorView error={tags.error} />;
  return (
    <section className={ui.page} aria-label="Tags">
      <h2 className={ui.sectionTitle}>Tags</h2>
      <div className={ui.tableScroll}>
        <table className={ui.table}>
          <thead>
            <tr>
              <th scope="col">Tag</th>
              <th scope="col">Posts</th>
              <th scope="col">Description</th>
            </tr>
          </thead>
          <tbody>
            {tags.data.map((t) => (
              <tr key={t.slug}>
                <td>
                  <Link to="/f/$owner/$slug/t/$tag" params={{ ...params, tag: t.slug }}>
                    <bdi>{cleanName(t.name)}</bdi>
                  </Link>
                  {t.unmergedEdits > 0 && (
                    <span className={ui.muted}> ({t.unmergedEdits} unmerged edits)</span>
                  )}
                </td>
                <td>{t.postCount}</td>
                <td className={ui.muted}>{t.description}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
