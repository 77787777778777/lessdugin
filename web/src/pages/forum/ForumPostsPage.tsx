// SPDX-License-Identifier: AGPL-3.0-or-later
// The forum's post list with ?sort, ?filter and ?tag (social.md §12.2–12.3).

import type { ListPostsQuery, PostFilter, PostSort } from "@lessdugin/engine/types";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { getRouteApi, Link } from "@tanstack/react-router";
import { Badges } from "../../components/Badges.tsx";
import { PostListItem } from "../../components/PostListItem.tsx";
import { ErrorView, Loading } from "../../components/Status.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
import { cx } from "../../lib/cx.ts";
import { cleanName } from "../../lib/text.ts";
import ui from "../../styles/ui.module.css";
import styles from "../pages.module.css";
import { useForumView } from "./forumContext.tsx";

const route = getRouteApi("/f/$owner/$slug");
const PAGE = 20;

const SORT_LABEL: Record<PostSort, string> = {
  magic: "Magic (new & upvoted)",
  top: "Top",
  new: "New",
  old: "Old",
  recentComments: "Recent comments",
};

const FILTER_LABEL: Record<PostFilter, string> = {
  frontpage: "Frontpage",
  all: "All posts",
  curated: "Curated",
};

export function ForumPostsPage() {
  const engine = useEngine();
  const view = useForumView();
  const params = route.useParams();
  const search = route.useSearch();
  const navigate = route.useNavigate();
  const sort = search.sort ?? "magic";
  const filter = search.filter ?? "frontpage";
  const tag = search.tag;
  const base: ListPostsQuery = {
    forum: view.forum.uri,
    sort,
    filter,
    limit: PAGE,
    ...(tag ? { tag } : {}),
  };
  const posts = useInfiniteQuery({
    queryKey: qk.posts(base),
    queryFn: ({ pageParam }) => engine.listPosts(pageParam ? { ...base, cursor: pageParam } : base),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.cursor,
  });
  const tags = useQuery({
    queryKey: qk.tags(view.forum.uri),
    queryFn: () => engine.tags(view.forum.uri),
  });

  const setSearch = (patch: {
    sort?: PostSort;
    filter?: PostFilter;
    tag?: string | undefined;
  }): void => {
    const next = { ...search, ...patch };
    void navigate({
      search: {
        ...(next.sort && next.sort !== "magic" ? { sort: next.sort } : {}),
        ...(next.filter && next.filter !== "frontpage" ? { filter: next.filter } : {}),
        ...(next.tag ? { tag: next.tag } : {}),
      },
    });
  };

  const items = posts.data?.pages.flatMap((p) => p.items) ?? [];
  const completeness = posts.data?.pages[0]?.completeness;
  return (
    <div className={styles.layout}>
      <section aria-label="Posts">
        <div className={styles.controls}>
          <fieldset className={ui.fieldsetRow}>
            <legend className="visually-hidden">Filter</legend>
            {(Object.keys(FILTER_LABEL) as PostFilter[]).map((f) => (
              <button
                key={f}
                type="button"
                className={filter === f ? ui.chipActive : ui.chip}
                aria-pressed={filter === f}
                onClick={() => setSearch({ filter: f })}
              >
                {FILTER_LABEL[f]}
              </button>
            ))}
          </fieldset>
          <label className={ui.row}>
            <span className={ui.muted}>Sort</span>
            <select
              className={styles.select}
              value={sort}
              onChange={(e) => setSearch({ sort: e.target.value as PostSort })}
            >
              {(Object.keys(SORT_LABEL) as PostSort[]).map((s) => (
                <option key={s} value={s}>
                  {SORT_LABEL[s]}
                </option>
              ))}
            </select>
          </label>
          {completeness && <Badges completeness={completeness} />}
        </div>
        {tag && (
          <p className={ui.row}>
            <span>
              Tagged <strong>{tag}</strong>
            </span>
            <button type="button" className={ui.link} onClick={() => setSearch({ tag: undefined })}>
              Clear tag filter
            </button>
          </p>
        )}
        {posts.isPending && <Loading />}
        {posts.isError && <ErrorView error={posts.error} />}
        {posts.data && items.length === 0 && <p className={ui.muted}>No posts match.</p>}
        <div>
          {items.map((p) => (
            <PostListItem key={p.uri} post={p} />
          ))}
        </div>
        {posts.hasNextPage && (
          <button
            type="button"
            className={ui.button}
            disabled={posts.isFetchingNextPage}
            onClick={() => void posts.fetchNextPage()}
          >
            {posts.isFetchingNextPage ? "Loading…" : "Load more"}
          </button>
        )}
      </section>
      <aside className={styles.aside} aria-label="Tags">
        <h2 className={ui.sectionTitle}>Tags</h2>
        <div className={ui.row}>
          {tags.data?.map((t) => (
            <button
              key={t.slug}
              type="button"
              className={cx(tag === t.slug ? ui.chipActive : ui.chip)}
              aria-pressed={tag === t.slug}
              onClick={() => setSearch({ tag: tag === t.slug ? undefined : t.slug })}
            >
              <bdi>{cleanName(t.name)}</bdi> <span className={ui.muted}>{t.postCount}</span>
            </button>
          ))}
        </div>
        <p>
          <Link to="/f/$owner/$slug/tags" params={params}>
            All tags →
          </Link>
        </p>
        {view.about && (
          <>
            <h2 className={ui.sectionTitle}>About this forum</h2>
            <p className={ui.small}>{view.about.excerpt}</p>
          </>
        )}
      </aside>
    </div>
  );
}
