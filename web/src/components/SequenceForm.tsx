// SPDX-License-Identifier: AGPL-3.0-or-later
// Writing a sequence (engine-api.md §3.5 `seq_create`, `seq_edit`; data-model.md §6.9): its
// title, introduction and chapters, each with a title, an introduction and its posts in order.
// Posts are added by link or at-uri (`parse_ref`); the posts may be by other authors. The draft
// replaces the sequence's editable content when it is saved, as one write.

import type { SequenceDraft } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { type FormEvent, useId, useRef, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import {
  type ChapterForm,
  EMPTY_CHAPTER,
  MAX_CHAPTER_POSTS,
  MAX_CHAPTERS,
  type SequenceForm,
  sequenceDraftOf,
} from "../lib/sequences.ts";
import { cleanName } from "../lib/text.ts";
import { parseAtUri } from "../lib/uri.ts";
import ui from "../styles/ui.module.css";
import { errorMessage } from "./Status.tsx";

/** A post entry's title (its summary), for the entry lists. */
export function EntryTitle({ uri }: { uri: string }) {
  const engine = useEngine();
  const q = useQuery({ queryKey: qk.post(uri), queryFn: () => engine.getPost(uri), retry: false });
  if (q.isPending) return <span className={ui.muted}>{parseAtUri(uri)?.rkey ?? uri}</span>;
  if (q.isError) return <span className={ui.muted}>Unavailable post</span>;
  const p = q.data.post;
  return <bdi>{cleanName(p.title ?? p.excerpt, 120)}</bdi>;
}

/** A field that adds a post to a chapter by its link or at-uri. `onAdd` resolves when the post
 * was taken; it may refuse by rejecting. */
export function AddPost({
  forum,
  label,
  onAdd,
}: {
  forum: string;
  label: string;
  onAdd: (uri: string) => Promise<void>;
}) {
  const engine = useEngine();
  const id = useId();
  const [input, setInput] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const add = async (): Promise<void> => {
    if (busy || input.trim() === "") return;
    setBusy(true);
    setProblem(null);
    try {
      const ref = await engine.parseRef(input.trim(), { forum });
      if (ref.kind !== "post") throw new Error("That address is not a post.");
      await onAdd(ref.uri);
      setInput("");
    } catch (e) {
      setProblem(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className={ui.field}>
      <label className={ui.label} htmlFor={id}>
        {label}
      </label>
      <div className={ui.row}>
        <input
          id={id}
          className={ui.input}
          value={input}
          placeholder="A post's link or at:// address"
          autoCapitalize="none"
          spellCheck={false}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void add();
            }
          }}
        />
        <button type="button" className={ui.button} disabled={busy} onClick={() => void add()}>
          Add
        </button>
      </div>
      {problem && (
        <p className={ui.error} role="alert">
          {problem}
        </p>
      )}
    </div>
  );
}

interface Keyed extends ChapterForm {
  key: number;
}

export function SequenceFormView({
  forum,
  initial,
  submitLabel,
  onSubmit,
}: {
  forum: string;
  initial: SequenceForm;
  submitLabel: string;
  /** Resolves when the write was accepted; a rejection is shown. */
  onSubmit: (draft: SequenceDraft) => Promise<void>;
}) {
  const ids = { title: useId(), body: useId() };
  const nextKey = useRef(0);
  const keyed = (c: ChapterForm): Keyed => ({ ...c, key: nextKey.current++ });
  const [title, setTitle] = useState(initial.title);
  const [markdown, setMarkdown] = useState(initial.markdown);
  const [chapters, setChapters] = useState<Keyed[]>(() => initial.chapters.map(keyed));
  const [problems, setProblems] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  const change = (i: number, f: (c: Keyed) => Keyed): void =>
    setChapters((prev) => prev.map((c, j) => (j === i ? f(c) : c)));
  const moveEntry = (i: number, k: number, by: -1 | 1): void =>
    change(i, (c) => {
      const posts = [...c.posts];
      const [p] = posts.splice(k, 1);
      if (p !== undefined) posts.splice(k + by, 0, p);
      return { ...c, posts };
    });

  const submit = async (e: FormEvent): Promise<void> => {
    e.preventDefault();
    if (busy) return;
    const found: string[] = [];
    if (title.trim() === "") found.push("A sequence needs a title.");
    if (chapters.length > MAX_CHAPTERS) found.push(`At most ${MAX_CHAPTERS} chapters.`);
    if (chapters.some((c) => c.posts.length > MAX_CHAPTER_POSTS)) {
      found.push(`At most ${MAX_CHAPTER_POSTS} posts in a chapter.`);
    }
    setProblems(found);
    if (found.length > 0) return;
    setBusy(true);
    try {
      await onSubmit(
        sequenceDraftOf(forum, { title, markdown, media: initial.media, chapters: chapters }),
      );
    } catch (err) {
      setProblems([errorMessage(err)]);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className={ui.stack} onSubmit={(e) => void submit(e)} noValidate>
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.title}>
          Title
        </label>
        <input
          id={ids.title}
          className={ui.input}
          value={title}
          maxLength={300}
          onChange={(e) => setTitle(e.target.value)}
          required
        />
      </div>
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.body}>
          Introduction <span className={ui.hint}>(optional, markdown)</span>
        </label>
        <textarea
          id={ids.body}
          className={ui.input}
          rows={5}
          value={markdown}
          onChange={(e) => setMarkdown(e.target.value)}
        />
      </div>
      {chapters.map((c, i) => (
        <ChapterFields
          key={c.key}
          n={i + 1}
          chapter={c}
          forum={forum}
          onChange={(f) => change(i, f)}
          onMove={(k, by) => moveEntry(i, k, by)}
          onRemove={() => setChapters((prev) => prev.filter((_, j) => j !== i))}
        />
      ))}
      <div className={ui.row}>
        <button
          type="button"
          className={ui.button}
          disabled={chapters.length >= MAX_CHAPTERS}
          onClick={() => setChapters((prev) => [...prev, keyed(EMPTY_CHAPTER)])}
        >
          Add chapter
        </button>
      </div>
      {problems.length > 0 && (
        <div className={ui.error} role="alert">
          <ul>
            {problems.map((p) => (
              <li key={p}>{p}</li>
            ))}
          </ul>
        </div>
      )}
      <p className={ui.small}>
        The sequence is a public record in your repository. It lists posts by their address, so it
        always shows their latest version.
      </p>
      <div className={ui.row}>
        <button type="submit" className={ui.primary} disabled={busy}>
          {submitLabel}
        </button>
      </div>
    </form>
  );
}

function ChapterFields({
  n,
  chapter,
  forum,
  onChange,
  onMove,
  onRemove,
}: {
  n: number;
  chapter: Keyed;
  forum: string;
  onChange: (f: (c: Keyed) => Keyed) => void;
  onMove: (k: number, by: -1 | 1) => void;
  onRemove: () => void;
}) {
  const ids = { title: useId(), body: useId() };
  return (
    <fieldset className={ui.card} aria-label={`Chapter ${n}`}>
      <legend className={ui.label}>Chapter {n}</legend>
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.title}>
          Chapter title <span className={ui.hint}>(optional)</span>
        </label>
        <input
          id={ids.title}
          className={ui.input}
          value={chapter.title}
          maxLength={300}
          onChange={(e) => {
            const title = e.target.value;
            onChange((c) => ({ ...c, title }));
          }}
        />
      </div>
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.body}>
          Chapter introduction <span className={ui.hint}>(optional, markdown)</span>
        </label>
        <textarea
          id={ids.body}
          className={ui.input}
          rows={3}
          value={chapter.markdown}
          onChange={(e) => {
            const markdown = e.target.value;
            onChange((c) => ({ ...c, markdown }));
          }}
        />
      </div>
      {chapter.posts.length > 0 && (
        <ol aria-label={`Posts of chapter ${n}`}>
          {chapter.posts.map((uri, k) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: an entry may repeat a post
            <li key={`${k}:${uri}`}>
              <EntryTitle uri={uri} />{" "}
              <span className={ui.row}>
                <button
                  type="button"
                  className={ui.ghost}
                  disabled={k === 0}
                  aria-label={`Move entry ${k + 1} up`}
                  onClick={() => onMove(k, -1)}
                >
                  ↑
                </button>
                <button
                  type="button"
                  className={ui.ghost}
                  disabled={k === chapter.posts.length - 1}
                  aria-label={`Move entry ${k + 1} down`}
                  onClick={() => onMove(k, 1)}
                >
                  ↓
                </button>
                <button
                  type="button"
                  className={ui.ghost}
                  aria-label={`Remove entry ${k + 1}`}
                  onClick={() =>
                    onChange((c) => ({ ...c, posts: c.posts.filter((_, j) => j !== k) }))
                  }
                >
                  Remove
                </button>
              </span>
            </li>
          ))}
        </ol>
      )}
      <AddPost
        forum={forum}
        label={`Add a post to chapter ${n}`}
        onAdd={async (uri) => {
          if (chapter.posts.length >= MAX_CHAPTER_POSTS) {
            throw new Error(`At most ${MAX_CHAPTER_POSTS} posts in a chapter.`);
          }
          onChange((c) => ({ ...c, posts: [...c.posts, uri] }));
        }}
      />
      <div className={ui.row}>
        <button type="button" className={ui.danger} onClick={onRemove}>
          Remove chapter {n}
        </button>
      </div>
    </fieldset>
  );
}
