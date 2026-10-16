// SPDX-License-Identifier: AGPL-3.0-or-later
import { useQuery } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { ActorName } from "../components/ActorName.tsx";
import { PostListItem } from "../components/PostListItem.tsx";
import { RenderedHtml } from "../components/RenderedHtml.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { useEngine } from "../engine/context.tsx";
import { qk, useParsedRef } from "../engine/queries.ts";
import { cleanName } from "../lib/text.ts";
import ui from "../styles/ui.module.css";

const route = getRouteApi("/s/$author/$rkey");

function Sequence({ uri }: { uri: string }) {
  const engine = useEngine();
  const q = useQuery({ queryKey: qk.sequence(uri), queryFn: () => engine.sequence(uri) });
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorView error={q.error} title="Sequence not found" />;
  const { sequence, body, chapters } = q.data;
  return (
    <article className={ui.page}>
      <h1 className={ui.pageTitle}>
        <bdi>{cleanName(sequence.title, 300)}</bdi>
      </h1>
      <p className={ui.small}>
        A sequence by <ActorName actor={sequence.author} showIdent /> · {sequence.score.score} karma
      </p>
      {body && <RenderedHtml html={body.html} />}
      {chapters.map((c, i) => (
        <section key={c.title ?? i} aria-label={c.title ?? `Chapter ${i + 1}`}>
          <h2 className={ui.sectionTitle}>{c.title ?? `Chapter ${i + 1}`}</h2>
          {c.body && <RenderedHtml html={c.body.html} className="lfm-compact" />}
          {c.posts.map((p) => (
            <PostListItem key={p.uri} post={p} />
          ))}
        </section>
      ))}
    </article>
  );
}

export function SequencePage() {
  const { author, rkey } = route.useParams();
  const ref = useParsedRef(`#/s/${author}/${rkey}`);
  if (ref.isPending) return <Loading />;
  if (ref.isError) return <ErrorView error={ref.error} />;
  if (ref.data.kind !== "sequence")
    return <ErrorView error={new Error("This address is not a sequence.")} />;
  return <Sequence uri={ref.data.uri} />;
}
