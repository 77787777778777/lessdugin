// SPDX-License-Identifier: AGPL-3.0-or-later
// Writing a post: kind, title, link (link posts), tags and the markdown body. Drafts are
// autosaved locally; `?draft=<id>` resumes one. `?edit=<uri>` edits one of the viewer's own
// posts (edit protocol, data-model.md §8.3): the form starts from the record, its media and
// labels are kept, and the previous version stays in the public history unless the author
// chooses to replace it without history.

import type { Draft, MediaRef, PostDraft, PostKind } from "@lessdugin/engine/types";
import type { Attachment } from "../../components/Attachments.tsx";
import { POST_KINDS } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { useCallback, useId, useRef, useState } from "react";
import { LazyComposer } from "../../components/LazyComposer.tsx";
import { KIND_LABEL } from "../../components/PostMeta.tsx";
import { ErrorView, Loading } from "../../components/Status.tsx";
import { toasts } from "../../components/Toast.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk, useActiveAccount } from "../../engine/queries.ts";
import { useAnnounceQueued } from "../../engine/queued.ts";
import { writeStore } from "../../engine/writes.ts";
import { capReason } from "../../lib/caps.ts";
import {
  markdownOf,
  mediaRefsOf,
  objectField,
  stringField,
  stringsField,
} from "../../lib/records.ts";
import { cleanName } from "../../lib/text.ts";
import { parseAtUri } from "../../lib/uri.ts";
import ui from "../../styles/ui.module.css";
import { useForumView } from "./forumContext.tsx";

const route = getRouteApi("/f/$owner/$slug/new");
const KINDS: PostKind[] = ["article", "question", "link", "quickTake"];

/** The post record being edited, as a draft (record JSON is untrusted: every field checked). */
export function postDraftOf(record: unknown, forum: string): PostDraft | null {
  const body = objectField(record, "body");
  const markdown = markdownOf(body);
  // An absent `kind` is `article` (data-model.md §6.3; writers omit the default). An unknown
  // kind is not edited here: rewriting it as another kind would change the post.
  const kind = stringField(record, "kind") ?? "article";
  if (markdown === undefined || !(POST_KINDS as readonly string[]).includes(kind)) {
    return null;
  }
  const d: PostDraft = {
    forum,
    kind: kind as PostKind,
    body: { markdown, media: mediaRefsOf(body) },
    tags: stringsField(record, "tags"),
    labels: stringsField(record, "labels"),
  };
  const title = stringField(record, "title");
  if (title !== undefined) d.title = title;
  const link = objectField(record, "link");
  const linkUri = stringField(link, "uri");
  if (linkUri !== undefined) {
    d.link = { uri: linkUri };
    const linkTitle = stringField(link, "title");
    if (linkTitle !== undefined) d.link.title = linkTitle;
    const description = stringField(link, "description");
    if (description !== undefined) d.link.description = description;
  }
  return d;
}

function PostForm({
  initial,
  draftId,
  editing,
}: {
  initial: PostDraft | null;
  draftId: string | undefined;
  /** The uri of the own post being edited (edit mode), if any. */
  editing?: string | undefined;
}) {
  const engine = useEngine();
  const { forum } = useForumView();
  const navigate = useNavigate();
  const announceQueued = useAnnounceQueued();
  const [kind, setKind] = useState<PostKind>(initial?.kind ?? "article");
  const [title, setTitle] = useState(initial?.title ?? "");
  const [link, setLink] = useState(initial?.link?.uri ?? "");
  const [tags, setTags] = useState<string[]>(initial?.tags ?? []);
  const [keepHistory, setKeepHistory] = useState(true);
  const [summary, setSummary] = useState("");
  const tagList = useQuery({ queryKey: qk.tags(forum.uri), queryFn: () => engine.tags(forum.uri) });
  const ids = { title: useId(), link: useId(), summary: useId() };
  const latest = useRef({ kind, title, link, tags });
  latest.current = { kind, title, link, tags };
  // Kept as they are: the composer edits only the markdown.
  const media = useRef<MediaRef[]>(initial?.body.media ?? []);
  const labels = useRef<string[]>(initial?.labels ?? []);
  const linkMeta = useRef(initial?.link);

  const draftFor = useCallback(
    (markdown: string): PostDraft => {
      const { kind: k, title: t, link: l, tags: tg } = latest.current;
      const d: PostDraft = {
        forum: forum.uri,
        kind: k,
        body: { markdown, media: media.current },
        tags: tg,
        labels: labels.current,
      };
      if (k !== "quickTake" && t.trim() !== "") d.title = t.trim();
      if (k === "link" && l.trim() !== "") {
        const prev = linkMeta.current;
        d.link = prev && prev.uri === l.trim() ? { ...prev } : { uri: l.trim() };
      }
      return d;
    },
    [forum.uri],
  );

  const makeDraft = useCallback(
    (markdown: string, id: string | undefined): Draft => ({
      kind: "post",
      ...(id !== undefined ? { id } : {}),
      post: draftFor(markdown),
    }),
    [draftFor],
  );

  const publish = async (markdown: string, attached: Attachment[]): Promise<boolean> => {
    // the prepared refs the composer collected (docs/plan.md M4); a refused file is never
    // already in the draft, and one whose row was removed is not either
    const prepared = attached.flatMap((a) => (a.kind === "ready" ? [a.ref] : []));
    media.current = [...media.current, ...prepared];
    const ticket = editing
      ? await engine.edit(
          editing,
          { kind: "post", post: draftFor(markdown) },
          {
            history: keepHistory,
            ...(keepHistory && summary.trim() !== "" ? { summary: summary.trim() } : {}),
          },
        )
      : await engine.publish(draftFor(markdown));
    writeStore.track(ticket, editing);
    const final = await writeStore.settle(ticket.id);
    if (final.kind === "failed") throw final.error;
    if (final.kind === "queued") {
      announceQueued(editing ? "Your edit" : "Your post", final.state);
      void navigate({ to: "/outbox" });
      return true;
    }
    if (final.kind !== "done") return false;
    toasts.show({ message: editing ? "Post updated" : "Published", tone: "success" });
    const p = parseAtUri(final.uri);
    if (p) void navigate({ to: "/p/$author/$rkey", params: { author: p.did, rkey: p.rkey } });
    return true;
  };

  return (
    <div className={ui.stack}>
      <fieldset className={ui.field}>
        <legend className={ui.label}>Kind</legend>
        <div className={ui.row}>
          {KINDS.map((k) => (
            <label key={k} className={kind === k ? ui.chipActive : ui.chip}>
              <input
                type="radio"
                name="post-kind"
                value={k}
                checked={kind === k}
                disabled={editing !== undefined && kind !== k}
                onChange={() => setKind(k)}
                className="visually-hidden"
              />
              {KIND_LABEL[k]}
            </label>
          ))}
        </div>
      </fieldset>
      {kind !== "quickTake" && (
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
      )}
      {kind === "link" && (
        <div className={ui.field}>
          <label className={ui.label} htmlFor={ids.link}>
            Link
          </label>
          <input
            id={ids.link}
            className={ui.input}
            type="url"
            inputMode="url"
            placeholder="https://"
            value={link}
            onChange={(e) => setLink(e.target.value)}
          />
          <span className={ui.hint}>
            The app never fetches link cards; add a summary in the body.
          </span>
        </div>
      )}
      <fieldset className={ui.field}>
        <legend className={ui.label}>Tags</legend>
        <div className={ui.row}>
          {tagList.data?.map((t) => (
            <label key={t.slug} className={tags.includes(t.slug) ? ui.chipActive : ui.chip}>
              <input
                type="checkbox"
                className="visually-hidden"
                checked={tags.includes(t.slug)}
                onChange={(e) =>
                  setTags((prev) =>
                    e.target.checked ? [...prev, t.slug] : prev.filter((x) => x !== t.slug),
                  )
                }
              />
              <bdi>{cleanName(t.name)}</bdi>
            </label>
          ))}
        </div>
      </fieldset>
      <div className={ui.field}>
        <span className={ui.label}>Body</span>
        <LazyComposer
          label="Post body"
          initial={initial?.body.markdown ?? ""}
          draftId={draftId}
          makeDraft={editing ? undefined : makeDraft}
          submitLabel={editing ? "Save edit" : "Publish"}
          placeholder="Write your post in markdown…"
          onSubmit={publish}
          attachments
        />
        <span className={ui.hint}>
          Published posts are public records in your repository. Edits keep a public history unless
          you redact it.
        </span>
      </div>
      {editing && (
        <fieldset className={ui.field}>
          <legend className={ui.label}>History</legend>
          <label className={ui.row}>
            <input
              type="checkbox"
              checked={keepHistory}
              onChange={(e) => setKeepHistory(e.target.checked)}
            />
            <span>Keep the previous version in the public history</span>
          </label>
          {keepHistory ? (
            <>
              <label className={ui.label} htmlFor={ids.summary}>
                Edit summary (optional, public)
              </label>
              <input
                id={ids.summary}
                className={ui.input}
                value={summary}
                maxLength={300}
                onChange={(e) => setSummary(e.target.value)}
              />
            </>
          ) : (
            <span className={ui.hint}>
              The new version replaces the old one without a revision record. Copies others have
              archived may remain.
            </span>
          )}
        </fieldset>
      )}
    </div>
  );
}

/** Edit mode: loads the viewer's own post record. */
function EditPost({ uri }: { uri: string }) {
  const engine = useEngine();
  const { forum } = useForumView();
  const active = useActiveAccount();
  const raw = useQuery({ queryKey: qk.raw(uri), queryFn: () => engine.raw(uri) });
  if (raw.isPending || active.isPending) return <Loading />;
  if (raw.isError) return <ErrorView error={raw.error} title="This post cannot be edited" />;
  const author = parseAtUri(uri)?.did;
  const draft = postDraftOf(raw.data.record, forum.uri);
  if (!draft || author === undefined || author !== active.data?.did) {
    return (
      <ErrorView
        error={new Error("Only the author can edit a post, and only one with an inline body.")}
        title="This post cannot be edited"
      />
    );
  }
  return (
    <section className={ui.page} aria-label="Edit post">
      <h2 className={ui.sectionTitle}>Edit post</h2>
      <PostForm key={uri} initial={draft} draftId={undefined} editing={uri} />
    </section>
  );
}

export function ForumNewPostPage() {
  const engine = useEngine();
  const view = useForumView();
  const { draft, edit } = route.useSearch();
  const active = useActiveAccount();
  const drafts = useQuery({
    queryKey: qk.drafts(),
    queryFn: () => engine.drafts(),
    enabled: draft !== undefined,
  });
  if (active.data === null) {
    return <p className={ui.notice}>Sign in to write a post.</p>;
  }
  if (edit !== undefined) return <EditPost uri={edit} />;
  if (draft !== undefined && drafts.isPending) return <Loading />;
  const entry = drafts.data?.find((d) => d.id === draft);
  const initial = entry?.draft.kind === "post" ? entry.draft.post : null;
  const viewer = view.viewer;
  // `post` cap false: the viewer's posts would not be listed (social.md §8). The fold would
  // refuse the write anyway; say so up front.
  if (viewer !== undefined && !viewer.post) {
    return (
      <section className={ui.page} aria-label="New post">
        <h2 className={ui.sectionTitle}>New post</h2>
        <p className={ui.notice}>{capReason(viewer, "post")}</p>
      </section>
    );
  }
  return (
    <section className={ui.page} aria-label="New post">
      <h2 className={ui.sectionTitle}>{initial ? "Continue draft" : "New post"}</h2>
      {viewer?.newcomer === true && (
        <p className={ui.notice} data-viewer-state="newcomer">
          You are a new user here: your post will be collapsed for other readers until you are
          established.
        </p>
      )}
      {viewer?.applicant === true && (
        <p className={ui.notice} data-viewer-state="applicant">
          Your membership is awaiting approval; your post stays unlisted until an authority grants
          it.
        </p>
      )}
      <PostForm key={draft ?? "new"} initial={initial} draftId={entry?.id} />
    </section>
  );
}
