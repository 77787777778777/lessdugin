// SPDX-License-Identifier: AGPL-3.0-or-later
// Writing a post: kind, title, link (link posts), tags and the markdown body. Drafts are
// autosaved locally; `?draft=<id>` resumes one. `?edit=<uri>` edits one of the viewer's own
// posts (edit protocol, data-model.md §8.3): the form starts from the record, its media and
// labels are kept, and the previous version stays in the public history unless the author
// chooses to replace it without history. A link post's card (title, description, preview
// image) is kept by the engine for the same link, unless the author removes it
// (`EditOpts.dropLinkCard`, engine-api.md §3.5 `edit`).

import type { Draft, MediaRef, PostDraft, PostKind } from "@lessdugin/engine/types";
import { POST_KINDS } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { useCallback, useId, useRef, useState } from "react";
import type { Attachment } from "../../components/Attachments.tsx";
import { LazyComposer } from "../../components/LazyComposer.tsx";
import { KIND_LABEL } from "../../components/PostMeta.tsx";
import { ErrorView, Loading } from "../../components/Status.tsx";
import { toasts } from "../../components/Toast.tsx";
import { useEngine } from "../../engine/context.tsx";
import { useCoherentEditSource } from "../../engine/editSource.ts";
import { qk, useActiveAccount } from "../../engine/queries.ts";
import { useAnnounceQueued } from "../../engine/queued.ts";
import { writeStore } from "../../engine/writes.ts";
import { capReason } from "../../lib/caps.ts";
import { suspendsNotice } from "../../lib/modActions.ts";
import {
  isSpilledBody,
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

/** Whether the record's link has a card: a title, a description or a preview image. */
export function hasLinkCard(record: unknown): boolean {
  const link = objectField(record, "link");
  return (
    stringField(link, "title") !== undefined ||
    stringField(link, "description") !== undefined ||
    objectField(link, "thumb") !== undefined
  );
}

/** The post record being edited, as a draft (record JSON is untrusted: every field checked).
 * `source`: the markdown of a body that spilled into a blob (engine-api.md §3.4 `body_source`),
 * which the record does not carry. */
export function postDraftOf(record: unknown, forum: string, source?: string): PostDraft | null {
  const body = objectField(record, "body");
  const markdown = markdownOf(body) ?? (isSpilledBody(body) ? source : undefined);
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

export function PostForm({
  initial,
  draftId,
  editing,
  suspends,
  linkCard = false,
}: {
  initial: PostDraft | null;
  draftId: string | undefined;
  /** The uri of the own post being edited (edit mode), if any. */
  editing?: string | undefined;
  /** Edit mode: the warning that the edit suspends the post's positive assertions. */
  suspends?: string | null | undefined;
  /** Edit mode: the record's link has a card, which the author may remove. */
  linkCard?: boolean;
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
  const [dropCard, setDropCard] = useState(false);
  const tagList = useQuery({ queryKey: qk.tags(forum.uri), queryFn: () => engine.tags(forum.uri) });
  const ids = { title: useId(), link: useId(), summary: useId(), dropCard: useId() };
  // the card can be removed while the link is the record's (another link has no card anyway)
  const cardShown =
    editing !== undefined && linkCard && kind === "link" && link.trim() === initial?.link?.uri;
  const removeCard = cardShown && dropCard;
  const latest = useRef({ kind, title, link, tags, removeCard });
  latest.current = { kind, title, link, tags, removeCard };
  // Kept as they are: the composer edits only the markdown.
  const media = useRef<MediaRef[]>(initial?.body.media ?? []);
  const [editMedia, setEditMedia] = useState<readonly MediaRef[]>(initial?.body.media ?? []);
  const labels = useRef<string[]>(initial?.labels ?? []);
  const linkMeta = useRef(initial?.link);

  const draftFor = useCallback(
    (markdown: string, withMedia: MediaRef[] = media.current): PostDraft => {
      const { kind: k, title: t, link: l, tags: tg, removeCard: rc } = latest.current;
      const d: PostDraft = {
        forum: forum.uri,
        kind: k,
        body: { markdown, media: withMedia },
        tags: tg,
        labels: labels.current,
      };
      if (k !== "quickTake" && t.trim() !== "") d.title = t.trim();
      if (k === "link" && l.trim() !== "") {
        const prev = linkMeta.current;
        d.link = prev && prev.uri === l.trim() && !rc ? { ...prev } : { uri: l.trim() };
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
    // This submission's media: the kept ones and the prepared ones. `media.current` takes them
    // only once the write is accepted: the composer keeps its attachments after a refusal or a
    // failure, and the engine does not deduplicate `body.media`, so a submission sent again
    // would otherwise list them twice.
    const withMedia = editing ? prepared : [...media.current, ...prepared];
    const ticket = editing
      ? await engine.edit(
          editing,
          { kind: "post", post: draftFor(markdown, withMedia) },
          {
            history: keepHistory,
            ...(keepHistory && summary.trim() !== "" ? { summary: summary.trim() } : {}),
            ...(removeCard ? { dropLinkCard: true } : {}),
          },
        )
      : await engine.publish(draftFor(markdown, withMedia));
    writeStore.track(ticket, editing);
    const final = await writeStore.settle(ticket.id);
    if (final.kind === "failed") throw final.error;
    if (final.kind === "queued") {
      media.current = withMedia;
      announceQueued(editing ? "Your edit" : "Your post", final.state);
      void navigate({ to: "/outbox" });
      return true;
    }
    if (final.kind !== "done") return false;
    media.current = withMedia;
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
          {cardShown && (
            <>
              <label className={ui.row}>
                <input
                  type="checkbox"
                  checked={dropCard}
                  onChange={(e) => setDropCard(e.target.checked)}
                  aria-describedby={ids.dropCard}
                />
                <span>Remove the link card</span>
              </label>
              <span id={ids.dropCard} className={ui.hint}>
                Its title, description and preview image are removed; the link stays.
              </span>
            </>
          )}
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
        {suspends && (
          <p className={ui.notice} role="note" data-testid="edit-suspends">
            {suspends}
          </p>
        )}
        <LazyComposer
          label="Post body"
          initial={initial?.body.markdown ?? ""}
          draftId={draftId}
          makeDraft={editing ? undefined : makeDraft}
          submitLabel={editing ? "Save edit" : "Publish"}
          placeholder="Write your post in markdown…"
          onSubmit={publish}
          attachments
          media={editMedia}
          initialAttachments={(initial?.body.media ?? []).map((ref) => ({
            kind: "ready",
            ref,
            name: ref.name ?? `Existing media ${ref.cid}`,
          }))}
          onAttachmentsChange={(next) => {
            if (editing) {
              const refs = next.flatMap((a) => (a.kind === "ready" ? [a.ref] : []));
              media.current = refs;
              setEditMedia(refs);
            }
          }}
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

/** Edit mode: loads the viewer's own post record (exported for its tests). */
export function EditPost({ uri }: { uri: string }) {
  const engine = useEngine();
  const { forum } = useForumView();
  const active = useActiveAccount();
  const source = useCoherentEditSource(engine, uri);
  // what the edit suspends, from the engine (social.md §4.5); an error means no warning
  const preflight = useQuery({
    queryKey: qk.editPreflight(uri),
    queryFn: () => engine.editPreflight(uri),
    retry: false,
  });
  const author = parseAtUri(uri)?.did;
  const own = author !== undefined && author === active.data?.did;
  if (source.isPending || active.isPending || preflight.isPending || source.isFetching) {
    return <Loading />;
  }
  if (source.isError) {
    return (
      <div className={ui.stack}>
        <ErrorView error={source.error} title="This post cannot be edited" />
        <div className={ui.row}>
          <button type="button" className={ui.button} onClick={source.retry}>
            Try again
          </button>
        </div>
      </div>
    );
  }
  if (!source.data) return <Loading />;
  const draft = postDraftOf(source.data.raw.record, forum.uri, source.data.source.markdown);
  if (!draft || !own) {
    const why = !own
      ? "Only the author can edit a post."
      : "This post's record is not one this app can edit: its body or its kind is unknown.";
    return <ErrorView error={new Error(why)} title="This post cannot be edited" />;
  }
  return (
    <section className={ui.page} aria-label="Edit post">
      <h2 className={ui.sectionTitle}>Edit post</h2>
      <PostForm
        key={uri}
        initial={draft}
        draftId={undefined}
        editing={uri}
        suspends={preflight.data ? suspendsNotice(preflight.data) : null}
        linkCard={hasLinkCard(source.data.raw.record)}
      />
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
