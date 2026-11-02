// SPDX-License-Identifier: AGPL-3.0-or-later
// A sequence (social.md §14): its introduction and chapters with their displayable entries. The
// author also edits it here: an entry moves up or down (`seq_move`) or out (`seq_remove`), a post
// is added to a chapter (`seq_add`), and "Edit sequence" opens the full editor (`seq_edit`).
// Positions are the record's (`raw`): the view leaves out the entries it cannot display.

import type { ChapterView, PostSummary, SequencePosition } from "@lessdugin/engine/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, Link } from "@tanstack/react-router";
import { useState } from "react";
import { ActorName } from "../components/ActorName.tsx";
import { PostListItem } from "../components/PostListItem.tsx";
import { RenderedHtml } from "../components/RenderedHtml.tsx";
import { AddPost } from "../components/SequenceForm.tsx";
import { ErrorView, errorMessage, Loading } from "../components/Status.tsx";
import { toasts } from "../components/Toast.tsx";
import { useEngine } from "../engine/context.tsx";
import { qk, useActiveAccount, useParsedRef } from "../engine/queries.ts";
import { useSendWrite } from "../engine/queued.ts";
import { stringField } from "../lib/records.ts";
import { type EntryMove, entryMove, MAX_CHAPTER_POSTS, recordChapters } from "../lib/sequences.ts";
import { cleanName } from "../lib/text.ts";
import { parseAtUri } from "../lib/uri.ts";
import ui from "../styles/ui.module.css";

const route = getRouteApi("/s/$author/$rkey");

/** The author's controls: each sends one write and then refreshes the sequence. */
function useEntryWrites(uri: string) {
  const engine = useEngine();
  const qc = useQueryClient();
  const send = useSendWrite();
  const [busy, setBusy] = useState(false);
  const run = async (start: () => Promise<Parameters<typeof send>[0]>, done: string) => {
    if (busy) return;
    setBusy(true);
    try {
      if ((await send(await start(), "Your sequence edit", uri)) !== null) {
        toasts.show({ message: done, tone: "success" });
      }
      void qc.invalidateQueries({ queryKey: qk.sequence(uri) });
      void qc.invalidateQueries({ queryKey: qk.raw(uri) });
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
    } finally {
      setBusy(false);
    }
  };
  return {
    busy,
    move: (from: SequencePosition, to: SequencePosition) =>
      run(() => engine.seqMove(uri, from, to), "Entry moved"),
    remove: (post: string) => run(() => engine.seqRemove(uri, post), "Removed from the sequence"),
    add: (post: string, at: SequencePosition) =>
      run(() => engine.seqAdd(uri, post, at), "Added to the sequence"),
  };
}

function EntryControls({
  post,
  moves,
  writes,
}: {
  post: PostSummary;
  moves: { up: EntryMove | undefined; down: EntryMove | undefined };
  writes: ReturnType<typeof useEntryWrites>;
}) {
  const name = cleanName(post.title ?? post.excerpt, 80);
  return (
    <div className={ui.row}>
      <button
        type="button"
        className={ui.ghost}
        disabled={writes.busy || !moves.up}
        aria-label={`Move ${name} up`}
        onClick={() => moves.up && void writes.move(moves.up.from, moves.up.to)}
      >
        ↑ Up
      </button>
      <button
        type="button"
        className={ui.ghost}
        disabled={writes.busy || !moves.down}
        aria-label={`Move ${name} down`}
        onClick={() => moves.down && void writes.move(moves.down.from, moves.down.to)}
      >
        ↓ Down
      </button>
      <button
        type="button"
        className={ui.ghost}
        disabled={writes.busy}
        aria-label={`Remove ${name} from the sequence`}
        onClick={() => void writes.remove(post.uri)}
      >
        Remove
      </button>
    </div>
  );
}

function Chapter({
  chapter,
  i,
  editing,
}: {
  chapter: ChapterView;
  i: number;
  /** The author's controls, with the record's entries of every chapter and the view's. */
  editing?:
    | {
        forum: string;
        record: string[][];
        listed: string[][];
        writes: ReturnType<typeof useEntryWrites>;
      }
    | undefined;
}) {
  const title = (chapter.title && cleanName(chapter.title, 300)) || `Chapter ${i + 1}`;
  return (
    <section aria-label={title}>
      <h2 className={ui.sectionTitle}>
        <bdi>{title}</bdi>
      </h2>
      {chapter.body && (
        <RenderedHtml html={chapter.body.html} media={chapter.body.media} className="lfm-compact" />
      )}
      {chapter.posts.map((p, k) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: a chapter may list a post twice
        <div key={`${k}:${p.uri}`}>
          <PostListItem post={p} />
          {editing && (
            <EntryControls
              post={p}
              moves={{
                up: entryMove(editing.record, editing.listed, i, k, "up"),
                down: entryMove(editing.record, editing.listed, i, k, "down"),
              }}
              writes={editing.writes}
            />
          )}
        </div>
      ))}
      {editing && (
        <AddPost
          forum={editing.forum}
          label={`Add a post to ${title}`}
          onAdd={async (post) => {
            const entries = editing.record[i]?.length ?? 0;
            if (entries >= MAX_CHAPTER_POSTS) {
              throw new Error(`At most ${MAX_CHAPTER_POSTS} posts in a chapter.`);
            }
            await editing.writes.add(post, { chapter: i, index: entries });
          }}
        />
      )}
    </section>
  );
}

function Sequence({ uri }: { uri: string }) {
  const engine = useEngine();
  const q = useQuery({ queryKey: qk.sequence(uri), queryFn: () => engine.sequence(uri) });
  const viewer = useActiveAccount().data?.did;
  const own = viewer !== undefined && parseAtUri(uri)?.did === viewer;
  const raw = useQuery({ queryKey: qk.raw(uri), queryFn: () => engine.raw(uri), enabled: own });
  const writes = useEntryWrites(uri);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorView page error={q.error} title="Sequence not found" />;
  const { sequence, body, chapters } = q.data;
  const p = parseAtUri(uri);
  const record = raw.data ? recordChapters(raw.data.record) : undefined;
  const forum = raw.data ? stringField(raw.data.record, "forum") : undefined;
  // the view keeps the record's chapters; out of step (a newer record), no controls until both
  // are refreshed
  const editing =
    own && record && forum && record.length === chapters.length
      ? { forum, record, listed: chapters.map((c) => c.posts.map((x) => x.uri)), writes }
      : undefined;
  return (
    <article className={ui.page}>
      <h1 className={ui.pageTitle}>
        <bdi>{cleanName(sequence.title, 300)}</bdi>
      </h1>
      <p className={ui.small}>
        A sequence by <ActorName actor={sequence.author} showIdent /> · {sequence.score.score} karma
      </p>
      {own && p && (
        <div className={ui.row}>
          <Link
            to="/s/$author/$rkey/edit"
            params={{ author: p.did, rkey: p.rkey }}
            className={ui.button}
          >
            Edit sequence
          </Link>
        </div>
      )}
      {body && <RenderedHtml html={body.html} media={body.media} />}
      {chapters.map((c, i) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: chapters have no identity but their order
        <Chapter key={i} chapter={c} i={i} editing={editing} />
      ))}
    </article>
  );
}

export function SequencePage() {
  const { author, rkey } = route.useParams();
  const ref = useParsedRef(`#/s/${author}/${rkey}`);
  if (ref.isPending) return <Loading />;
  if (ref.isError) return <ErrorView page error={ref.error} />;
  if (ref.data.kind !== "sequence")
    return <ErrorView page error={new Error("This address is not a sequence.")} />;
  return <Sequence uri={ref.data.uri} />;
}
