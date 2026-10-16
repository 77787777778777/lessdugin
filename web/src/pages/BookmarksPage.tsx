// SPDX-License-Identifier: AGPL-3.0-or-later
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { PostListItem } from "../components/PostListItem.tsx";
import { RecordLink } from "../components/RecordLink.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import ui from "../styles/ui.module.css";

export function BookmarksPage() {
  const engine = useEngine();
  const qc = useQueryClient();
  const q = useQuery({ queryKey: qk.bookmarks(), queryFn: () => engine.bookmarks() });
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Bookmarks</h1>
      <p className={ui.muted}>Bookmarks are private to this device.</p>
      {q.isPending && <Loading />}
      {q.isError && <ErrorView error={q.error} />}
      {q.data && q.data.length === 0 && <p>No bookmarks.</p>}
      {q.data?.map((b) => (
        <div key={b.uri}>
          {b.post ? <PostListItem post={b.post} /> : <RecordLink uri={b.uri}>{b.uri}</RecordLink>}
          <button
            type="button"
            className={ui.ghost}
            onClick={() =>
              void engine
                .bookmark(b.uri, false)
                .then(() => qc.invalidateQueries({ queryKey: qk.bookmarks() }))
            }
          >
            Remove bookmark
          </button>
        </div>
      ))}
    </div>
  );
}
