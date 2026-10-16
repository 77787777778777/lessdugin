// SPDX-License-Identifier: AGPL-3.0-or-later
// Local search over synced, listed items. Snippets are plain text; highlights are code-point
// ranges into the snippet, rendered with <mark>.

import { isCommentNode, type SearchHit } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { type ReactNode, useEffect, useState } from "react";
import { ActorName } from "../components/ActorName.tsx";
import { postTitle } from "../components/PostMeta.tsx";
import { RecordLink } from "../components/RecordLink.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { useEngine } from "../engine/context.tsx";
import { useCurrentForum } from "../engine/currentForum.ts";
import { qk } from "../engine/queries.ts";
import ui from "../styles/ui.module.css";
import styles from "./pages.module.css";

const route = getRouteApi("/search");

export function highlightSnippet(snippet: string, ranges: [number, number][]): ReactNode[] {
  const cps = Array.from(snippet);
  const out: ReactNode[] = [];
  let at = 0;
  const sorted = [...ranges].sort((a, b) => a[0] - b[0]);
  for (const [s, e] of sorted) {
    if (s < at || e <= s) continue;
    if (s > at) out.push(cps.slice(at, s).join(""));
    out.push(<mark key={`${s}-${e}`}>{cps.slice(s, e).join("")}</mark>);
    at = e;
  }
  if (at < cps.length) out.push(cps.slice(at).join(""));
  return out;
}

function Hit({ hit }: { hit: SearchHit }) {
  const item = hit.item;
  return (
    <li className={styles.discussion}>
      {isCommentNode(item) ? (
        <p className={ui.small}>
          Comment by <ActorName actor={item.author} /> ·{" "}
          <RecordLink uri={item.uri}>open</RecordLink>
        </p>
      ) : (
        <h2 className={styles.discussionTitle}>
          <RecordLink uri={item.uri}>
            <bdi>{postTitle(item)}</bdi>
          </RecordLink>
        </h2>
      )}
      <p className={styles.snippet}>{highlightSnippet(hit.snippet, hit.highlights)}</p>
    </li>
  );
}

export function SearchPage() {
  const engine = useEngine();
  const { q = "" } = route.useSearch();
  const navigate = route.useNavigate();
  const forum = useCurrentForum() ?? undefined;
  const [text, setText] = useState(q);
  useEffect(() => setText(q), [q]);
  const results = useQuery({
    queryKey: qk.search(q, forum),
    queryFn: () => engine.search({ q, limit: 50, ...(forum ? { forum } : {}) }),
    enabled: q.trim() !== "",
  });
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Search</h1>
      <search>
        <form
          className={ui.row}
          onSubmit={(e) => {
            e.preventDefault();
            void navigate({ search: text.trim() === "" ? {} : { q: text.trim() } });
          }}
        >
          <label className={ui.field}>
            <span className="visually-hidden">Search terms</span>
            <input
              className={ui.input}
              type="search"
              value={text}
              onChange={(e) => setText(e.target.value)}
            />
          </label>
          <button type="submit" className={ui.primary}>
            Search
          </button>
        </form>
      </search>
      {results.isFetching && <Loading label="Searching…" />}
      {results.isError && <ErrorView error={results.error} />}
      {results.data && (
        <>
          <p className={ui.muted}>
            {results.data.hits.length} {results.data.hits.length === 1 ? "result" : "results"}
          </p>
          <ul className={ui.list}>
            {results.data.hits.map((h) => (
              <Hit key={h.item.uri} hit={h} />
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
