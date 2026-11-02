// SPDX-License-Identifier: AGPL-3.0-or-later
// A wiki page's history (social.md §13.2 "History view"): the chain from the canonical revision,
// following at each step the parent with the greatest depth, and the count of unmerged edits.
// Each revision links to its raw record, which shows its text.

import type { ActorView } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { getRouteApi, Link } from "@tanstack/react-router";
import { shortDid } from "../../components/AccountName.tsx";
import { ActorName } from "../../components/ActorName.tsx";
import { RecordLink } from "../../components/RecordLink.tsx";
import { ErrorView, Loading } from "../../components/Status.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
import { cleanName, plural } from "../../lib/text.ts";
import { fullDate } from "../../lib/time.ts";
import { parseAtUri } from "../../lib/uri.ts";
import { loadWikiNodes, WIKI_HISTORY_MAX, type WikiStep, wikiChain } from "../../lib/wiki.ts";
import ui from "../../styles/ui.module.css";
import { forumParams, useForumView } from "./forumContext.tsx";

const route = getRouteApi("/f/$owner/$slug/t/$tag/history");

function RevisionAuthor({ did }: { did: string }) {
  const engine = useEngine();
  const q = useQuery({
    queryKey: qk.actor(did),
    queryFn: () => engine.resolveActor(did),
    staleTime: 5 * 60_000,
  });
  const actor: ActorView = q.data ?? { did, handleState: "unverified", shortDid: shortDid(did) };
  return <ActorName actor={actor} />;
}

function Revision({ step, n, current }: { step: WikiStep; n: number; current: boolean }) {
  const record = step.node.raw?.record;
  const field = (k: string): string | undefined => {
    const v = (record as Record<string, unknown> | null | undefined)?.[k];
    return typeof v === "string" ? v : undefined;
  };
  const at = Date.parse(field("createdAt") ?? "");
  const summary = field("summary");
  const author = parseAtUri(step.node.uri)?.did;
  return (
    <li aria-label={`Revision ${n}`}>
      <p>
        <RecordLink uri={step.node.uri}>Revision {n}</RecordLink>
        {current && <strong> (current)</strong>} · <bdi>{cleanName(field("name") ?? "", 100)}</bdi>
        {author && (
          <>
            {" "}
            by <RevisionAuthor did={author} />
          </>
        )}
        {Number.isFinite(at) && (
          <>
            {" "}
            · <time dateTime={new Date(at).toISOString()}>{fullDate(at)}</time>
          </>
        )}
      </p>
      {summary !== undefined && summary !== "" && (
        <p className={ui.small}>
          <bdi>{cleanName(summary, 300)}</bdi>
        </p>
      )}
      {step.altered && (
        <p className={ui.small} role="note">
          Ancestor altered: the next revision names a different version of this one.
        </p>
      )}
    </li>
  );
}

export function WikiHistoryPage() {
  const engine = useEngine();
  const { forum } = useForumView();
  const { tag } = route.useParams();
  const view = useQuery({
    queryKey: qk.tag(forum.uri, tag),
    queryFn: () => engine.tag(forum.uri, tag),
  });
  const head = view.data?.head;
  const nodes = useQuery({
    // under the tag's key, so what refreshes the page refreshes its history
    queryKey: [...qk.tag(forum.uri, tag), "history", head ?? null],
    queryFn: () => loadWikiNodes((uri) => engine.raw(uri), head ?? ""),
    enabled: head !== undefined,
  });
  if (view.isPending) return <Loading />;
  if (view.isError) return <ErrorView error={view.error} title="Page not found" />;
  if (head === undefined) return <p className={ui.muted}>This page has no revisions yet.</p>;
  if (nodes.isPending) return <Loading label="Loading the history…" />;
  if (nodes.isError) return <ErrorView error={nodes.error} />;
  const chain = wikiChain(head, nodes.data);
  const unmerged = view.data.tag.unmergedEdits;
  return (
    <section className={ui.page} aria-label={`History of ${tag}`}>
      <div className={ui.spread}>
        <h2 className={ui.sectionTitle}>
          History of <bdi>{cleanName(view.data.tag.name)}</bdi>
        </h2>
        <Link
          to="/f/$owner/$slug/t/$tag"
          params={{ ...forumParams(forum.uri), tag }}
          className={ui.button}
        >
          Back to the page
        </Link>
      </div>
      {unmerged > 0 && (
        <p className={ui.notice}>
          {plural(unmerged, "unmerged edit")}: other revisions that are not part of this history.
        </p>
      )}
      <ol className={ui.list} aria-label="Revisions">
        {chain.steps.map((s, i) => (
          <Revision key={s.node.uri} step={s} n={chain.steps.length - i} current={i === 0} />
        ))}
      </ol>
      {chain.parentUnavailable && (
        <p className={ui.muted}>
          Earlier revisions are unavailable: deleted, hidden, invalid or not synced.
        </p>
      )}
      {nodes.data.size >= WIKI_HISTORY_MAX && (
        <p className={ui.muted}>Only the newest {WIKI_HISTORY_MAX} revisions are read.</p>
      )}
    </section>
  );
}
