// SPDX-License-Identifier: AGPL-3.0-or-later
// Editing an own sequence (engine-api.md §3.5 `seq_*`, social.md §14, data-model.md §6.9). An
// edit starts from the record (`raw`), not from the `SequenceView`: the view lists only the
// displayable entries, so a draft built from it would drop the hidden and unavailable ones, and
// its positions would not be the record's. Record JSON is untrusted: every field is checked.

import type { MediaRef, SequenceDraft, SequencePosition } from "@lessdugin/engine/types";
import { isSpilledBody, markdownOf, mediaRefsOf, objectField, stringField } from "./records.ts";

/** The lexicon's caps (`sequence.chapters` maxLength 50, `#chapter.posts` maxLength 100). */
export const MAX_CHAPTERS = 50;
export const MAX_CHAPTER_POSTS = 100;

export interface ChapterForm {
  title: string;
  markdown: string;
  media: MediaRef[];
  posts: string[];
}

export interface SequenceForm {
  title: string;
  markdown: string;
  media: MediaRef[];
  chapters: ChapterForm[];
}

export const EMPTY_CHAPTER: ChapterForm = { title: "", markdown: "", media: [], posts: [] };

export const EMPTY_SEQUENCE_FORM: SequenceForm = {
  title: "",
  markdown: "",
  media: [],
  chapters: [EMPTY_CHAPTER],
};

/** A body's markdown and media, or null when it spilled into a blob (its source is not in the
 * record, and a draft sent without it would drop the text). */
function bodyOf(body: unknown): { markdown: string; media: MediaRef[] } | null {
  if (body !== undefined && isSpilledBody(body)) return null;
  return { markdown: markdownOf(body) ?? "", media: mediaRefsOf(body) };
}

/** The editable content of a sequence record, or null when a body cannot be edited here. */
export function sequenceFormOf(record: unknown): SequenceForm | null {
  const body = bodyOf(objectField(record, "body"));
  if (!body) return null;
  const list = (record as { chapters?: unknown } | null)?.chapters;
  const chapters: ChapterForm[] = [];
  for (const c of Array.isArray(list) ? list : []) {
    const cb = bodyOf(objectField(c, "body"));
    if (!cb) return null;
    const posts = (c as { posts?: unknown } | null)?.posts;
    chapters.push({
      title: stringField(c, "title") ?? "",
      markdown: cb.markdown,
      media: cb.media,
      posts: Array.isArray(posts) ? posts.filter((p): p is string => typeof p === "string") : [],
    });
  }
  return { title: stringField(record, "title") ?? "", ...body, chapters };
}

/** The draft `seq_create` / `seq_edit` take for a form. An empty body is left out (an edit then
 * removes it), unless it still carries media. */
export function sequenceDraftOf(forum: string, form: SequenceForm): SequenceDraft {
  const d: SequenceDraft = {
    forum,
    title: form.title.trim(),
    chapters: form.chapters.map((c) => {
      const ch: SequenceDraft["chapters"][number] = { posts: [...c.posts] };
      if (c.title.trim() !== "") ch.title = c.title.trim();
      if (c.markdown.trim() !== "" || c.media.length > 0) {
        ch.body = { markdown: c.markdown, media: c.media };
      }
      return ch;
    }),
  };
  if (form.markdown.trim() !== "" || form.media.length > 0) {
    d.body = { markdown: form.markdown, media: form.media };
  }
  return d;
}

export interface EntryMove {
  from: SequencePosition;
  to: SequencePosition;
}

/** The record positions of the entries a view chapter lists: the view keeps the record's order
 * and leaves out the entries it cannot display, so its k-th entry is the record's next entry
 * naming the same post. -1 when the record does not have it (they are out of step). */
export function recordIndices(recordPosts: readonly string[], listed: readonly string[]): number[] {
  const out: number[] = [];
  let j = 0;
  for (const uri of listed) {
    while (j < recordPosts.length && recordPosts[j] !== uri) j++;
    out.push(j < recordPosts.length ? j : -1);
    j++;
  }
  return out;
}

/** The `seq_move` that moves the k-th listed entry of `chapter` one place up or down among the
 * listed entries (past the entries the view leaves out), or to the end of the chapter before /
 * the start of the chapter after when it is the first / last one. `seq_move` removes the entry
 * before it reads the destination index (organisation.rs). Absent when there is nowhere to go. */
export function entryMove(
  record: readonly (readonly string[])[],
  listed: readonly (readonly string[])[],
  chapter: number,
  k: number,
  dir: "up" | "down",
): EntryMove | undefined {
  const posts = record[chapter];
  const shown = listed[chapter];
  if (!posts || !shown) return undefined;
  const idx = recordIndices(posts, shown);
  const at = idx[k];
  if (at === undefined || at < 0) return undefined;
  const from = { chapter, index: at };
  if (dir === "up") {
    const prev = idx[k - 1];
    if (prev !== undefined && prev >= 0) return { from, to: { chapter, index: prev } };
    const before = record[chapter - 1];
    if (k === 0 && before) return { from, to: { chapter: chapter - 1, index: before.length } };
    return undefined;
  }
  const next = idx[k + 1];
  if (next !== undefined && next >= 0) return { from, to: { chapter, index: next } };
  if (k === shown.length - 1 && record[chapter + 1]) {
    return { from, to: { chapter: chapter + 1, index: 0 } };
  }
  return undefined;
}

/** The record's chapters' entries (`raw(uri).record`), for positions. */
export function recordChapters(record: unknown): string[][] {
  const list = (record as { chapters?: unknown } | null)?.chapters;
  return (Array.isArray(list) ? list : []).map((c) => {
    const posts = (c as { posts?: unknown } | null)?.posts;
    return Array.isArray(posts) ? posts.filter((p): p is string => typeof p === "string") : [];
  });
}
