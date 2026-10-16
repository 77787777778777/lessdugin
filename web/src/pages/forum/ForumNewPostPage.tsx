// SPDX-License-Identifier: AGPL-3.0-or-later
// Writing a post: kind, title, link (link posts), tags and the markdown body. Drafts are
// autosaved locally; `?draft=<id>` resumes one.

import type { Draft, PostDraft, PostKind } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { useCallback, useId, useRef, useState } from "react";
import { LazyComposer } from "../../components/LazyComposer.tsx";
import { KIND_LABEL } from "../../components/PostMeta.tsx";
import { Loading } from "../../components/Status.tsx";
import { toasts } from "../../components/Toast.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk, useActiveAccount } from "../../engine/queries.ts";
import { writeStore } from "../../engine/writes.ts";
import { parseAtUri } from "../../lib/uri.ts";
import ui from "../../styles/ui.module.css";
import { useForumView } from "./forumContext.tsx";

const route = getRouteApi("/f/$owner/$slug/new");
const KINDS: PostKind[] = ["article", "question", "link", "quickTake"];

function PostForm({
  initial,
  draftId,
}: {
  initial: PostDraft | null;
  draftId: string | undefined;
}) {
  const engine = useEngine();
  const { forum } = useForumView();
  const navigate = useNavigate();
  const [kind, setKind] = useState<PostKind>(initial?.kind ?? "article");
  const [title, setTitle] = useState(initial?.title ?? "");
  const [link, setLink] = useState(initial?.link?.uri ?? "");
  const [tags, setTags] = useState<string[]>(initial?.tags ?? []);
  const tagList = useQuery({ queryKey: qk.tags(forum.uri), queryFn: () => engine.tags(forum.uri) });
  const ids = { title: useId(), link: useId() };
  const latest = useRef({ kind, title, link, tags });
  latest.current = { kind, title, link, tags };

  const draftFor = useCallback(
    (markdown: string): PostDraft => {
      const { kind: k, title: t, link: l, tags: tg } = latest.current;
      const d: PostDraft = {
        forum: forum.uri,
        kind: k,
        body: { markdown, media: [] },
        tags: tg,
        labels: [],
      };
      if (k !== "quickTake" && t.trim() !== "") d.title = t.trim();
      if (k === "link" && l.trim() !== "") d.link = { uri: l.trim() };
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

  const publish = async (markdown: string): Promise<boolean> => {
    const ticket = await engine.publish(draftFor(markdown));
    writeStore.track(ticket);
    const final = await writeStore.wait(ticket.id);
    if (final.kind === "failed") throw final.error;
    if (final.kind !== "done") return false;
    toasts.show({ message: "Published", tone: "success" });
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
              {t.name}
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
          makeDraft={makeDraft}
          submitLabel="Publish"
          placeholder="Write your post in markdown…"
          onSubmit={publish}
        />
        <span className={ui.hint}>
          Published posts are public records in your repository. Edits keep a public history unless
          you redact it.
        </span>
      </div>
    </div>
  );
}

export function ForumNewPostPage() {
  const engine = useEngine();
  const { draft } = route.useSearch();
  const active = useActiveAccount();
  const drafts = useQuery({
    queryKey: qk.drafts(),
    queryFn: () => engine.drafts(),
    enabled: draft !== undefined,
  });
  if (active.data === null) {
    return <p className={ui.notice}>Sign in to write a post.</p>;
  }
  if (draft !== undefined && drafts.isPending) return <Loading />;
  const entry = drafts.data?.find((d) => d.id === draft);
  const initial = entry?.draft.kind === "post" ? entry.draft.post : null;
  return (
    <section className={ui.page} aria-label="New post">
      <h2 className={ui.sectionTitle}>{initial ? "Continue draft" : "New post"}</h2>
      <PostForm key={draft ?? "new"} initial={initial} draftId={entry?.id} />
    </section>
  );
}
