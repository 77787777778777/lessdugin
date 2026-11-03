// SPDX-License-Identifier: AGPL-3.0-or-later
// Writing a tag's wiki page (engine-api.md §3.5 `wiki_edit`, social.md §13, data-model.md §6.8).
// Every save is a new immutable revision. An edit starts from the canonical revision the page
// shows (`TagView.head`, its record read with `raw`) and names it as `base` (decision O-2): the
// engine writes it as the only parent and refuses the edit when the page's head has moved since,
// so an edit made against an older head never silently replaces a newer one. The editor keeps
// the head it loaded while the author writes; when the head moves (or a save is refused), the
// author's text stays, and "Reload the page" loads the newer version and keeps their text
// beside it so they can redo the edit. A page with no revision yet starts empty and is written
// without a parent. The engine opens only applicable pages (`tag` is not_found for a page that
// is not a tag, views.rs `tag_summary`), so such a page cannot be loaded here: the editor says
// so for a new address, and before a page stops being a tag.

import type { WikiDraft } from "@lessdugin/engine/types";
import { isEngineError } from "@lessdugin/engine/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { type FormEvent, useId, useState } from "react";
import { SLUG_RE } from "../../components/ForumForm.tsx";
import { ErrorView, errorMessage, Loading } from "../../components/Status.tsx";
import { toasts } from "../../components/Toast.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk, useActiveAccount } from "../../engine/queries.ts";
import { useSendWrite } from "../../engine/queued.ts";
import { cleanName } from "../../lib/text.ts";
import { EMPTY_WIKI_FORM, type WikiForm, wikiFormOf } from "../../lib/wiki.ts";
import ui from "../../styles/ui.module.css";
import { forumParams, useForumView } from "./forumContext.tsx";

const route = getRouteApi("/f/$owner/$slug/t/$tag/edit");

/** `wiki_edit`'s refusal of an edit whose base is no longer the head (decision O-2). */
export function isHeadMoved(e: unknown): boolean {
  return isEngineError(e) && e.code === "conflict" && e.detail?.kind === "wikiHeadMoved";
}

export const HEAD_MOVED_MESSAGE =
  "This page changed since you opened it. Reload it to see the new version, then make your edit again.";

/** Shown with the author's text after a reload. */
export const CARRIED_NOTE =
  "Your edit was not saved: the editor now shows the newer version of the page. Your text is " +
  "kept here so you can make your edit again.";

/** Shown for an address the engine has no tag page for: it may still have a page that is not a
 * tag, which the edit then continues (`wiki_edit` without a base names the current head). */
export const NEW_PAGE_NOTE =
  "Only pages that are tags open here. If this address already has a page that is not a tag, " +
  "what you save becomes its next revision, in place of its current name, text and parent tags " +
  "(its history keeps them).";

/** Shown while "This page is a tag" is unticked. */
export const NOT_A_TAG_WARNING =
  "Once saved, a page that is not a tag leaves the tags list and no longer applies to posts, " +
  "and this app cannot open it again: an edit here would start from an empty page.";

/** The form (exported for its tests). `base`: the head the form started from, absent for a page
 * with no revision yet. `moved`: the page's head is no longer `base` (or a page appeared where
 * there was none). `onReload` gets the author's current text, and `carried` is the text a
 * reload kept. */
export function WikiPageForm({
  slug,
  base,
  initial,
  moved: headMoved = false,
  onReload,
  carried = null,
  onDismissCarried,
}: {
  slug: string;
  base: string | undefined;
  initial: WikiForm;
  moved?: boolean;
  onReload?: (mine: WikiForm) => void;
  carried?: WikiForm | null;
  onDismissCarried?: () => void;
}) {
  const engine = useEngine();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const send = useSendWrite();
  const { forum } = useForumView();
  const ids = {
    name: useId(),
    description: useId(),
    body: useId(),
    summary: useId(),
    notTag: useId(),
    carried: useId(),
  };
  const [name, setName] = useState(initial.name);
  const [description, setDescription] = useState(initial.description);
  const [markdown, setMarkdown] = useState(initial.markdown);
  const [isTag, setIsTag] = useState(initial.isTag);
  const [parentTags, setParentTags] = useState<string[]>(initial.parentTags);
  const [summary, setSummary] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [moved, setMoved] = useState(false);
  const [busy, setBusy] = useState(false);
  const tags = useQuery({ queryKey: qk.tags(forum.uri), queryFn: () => engine.tags(forum.uri) });
  // the forum's tags, and any parent the record names that is not one (kept unless unticked)
  const choices = [
    ...(tags.data ?? [])
      .filter((t) => t.slug !== slug)
      .map((t) => ({ slug: t.slug, name: t.name })),
    ...initial.parentTags
      .filter((p) => !(tags.data ?? []).some((t) => t.slug === p))
      .map((p) => ({ slug: p, name: p })),
  ];

  const submit = async (e: FormEvent): Promise<void> => {
    e.preventDefault();
    if (busy) return;
    if (name.trim() === "") {
      setProblem("A page needs a name.");
      return;
    }
    // `base`: the head this form started from (engine-api.md §3.5 `wiki_edit`, O-2)
    const draft: WikiDraft = {
      name: name.trim(),
      isTag,
      parentTags,
    };
    if (description.trim() !== "") draft.description = description.trim();
    if (markdown.trim() !== "" || initial.media.length > 0) {
      draft.body = { markdown, media: initial.media };
    }
    if (summary.trim() !== "") draft.summary = summary.trim();
    if (base !== undefined) draft.base = base;
    setBusy(true);
    setProblem(null);
    try {
      const done = await send(await engine.wikiEdit(forum.uri, slug, draft), "Your page edit");
      if (done === null) {
        void navigate({ to: "/outbox" });
        return;
      }
      void qc.invalidateQueries({ queryKey: qk.tag(forum.uri, slug) });
      void qc.invalidateQueries({ queryKey: qk.tags(forum.uri) });
      if (!isTag) {
        // the engine has no tag page for it: back to the tags list, which no longer lists it
        toasts.show({
          message: "Page saved (a public revision). It is not a tag, so it is not listed.",
          tone: "success",
        });
        void navigate({ to: "/f/$owner/$slug/tags", params: forumParams(forum.uri) });
        return;
      }
      toasts.show({ message: "Page saved (a public revision)", tone: "success" });
      void navigate({
        to: "/f/$owner/$slug/t/$tag",
        params: { ...forumParams(forum.uri), tag: slug },
      });
    } catch (err) {
      setMoved(isHeadMoved(err));
      setProblem(isHeadMoved(err) ? HEAD_MOVED_MESSAGE : errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const mine = (): WikiForm => ({
    name,
    description,
    markdown,
    media: initial.media,
    isTag,
    parentTags,
  });
  const reload = (): void => {
    if (onReload) onReload(mine());
    else void qc.invalidateQueries({ queryKey: qk.tag(forum.uri, slug) });
  };

  return (
    <form className={ui.stack} onSubmit={(e) => void submit(e)} noValidate>
      {carried && (
        <section className={ui.notice} aria-labelledby={ids.carried}>
          <h3 id={ids.carried} className={ui.label}>
            Your unsaved edit
          </h3>
          <p>{CARRIED_NOTE}</p>
          {carried.name !== initial.name && (
            <p>
              Your name: <bdi>{cleanName(carried.name)}</bdi>
            </p>
          )}
          {carried.description !== initial.description && (
            <p>
              Your description: <bdi>{carried.description}</bdi>
            </p>
          )}
          <label className={ui.label}>
            Your unsaved text
            <textarea className={ui.input} rows={8} value={carried.markdown} readOnly />
          </label>
          <div className={ui.row}>
            <button
              type="button"
              className={ui.button}
              onClick={() => {
                setName(carried.name);
                setDescription(carried.description);
                setMarkdown(carried.markdown);
              }}
            >
              Use my text
            </button>
            <button type="button" className={ui.ghost} onClick={() => onDismissCarried?.()}>
              Dismiss
            </button>
          </div>
        </section>
      )}
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.name}>
          Name
        </label>
        <input
          id={ids.name}
          className={ui.input}
          value={name}
          maxLength={100}
          onChange={(e) => setName(e.target.value)}
          required
        />
      </div>
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.description}>
          Description <span className={ui.hint}>(optional, plain text)</span>
        </label>
        <input
          id={ids.description}
          className={ui.input}
          value={description}
          maxLength={300}
          onChange={(e) => setDescription(e.target.value)}
        />
      </div>
      <label className={ui.row}>
        <input
          type="checkbox"
          checked={isTag}
          onChange={(e) => setIsTag(e.target.checked)}
          aria-describedby={isTag ? undefined : ids.notTag}
        />
        <span>
          This page is a tag <span className={ui.hint}>(posts can be tagged with it)</span>
        </span>
      </label>
      <div aria-live="polite">
        {!isTag && (
          <p id={ids.notTag} className={ui.notice}>
            {NOT_A_TAG_WARNING}
          </p>
        )}
      </div>
      {choices.length > 0 && (
        <fieldset className={ui.field}>
          <legend className={ui.label}>Parent tags</legend>
          <div className={ui.row}>
            {choices.map((t) => (
              <label key={t.slug} className={parentTags.includes(t.slug) ? ui.chipActive : ui.chip}>
                <input
                  type="checkbox"
                  className="visually-hidden"
                  checked={parentTags.includes(t.slug)}
                  onChange={(e) =>
                    setParentTags((prev) =>
                      e.target.checked ? [...prev, t.slug] : prev.filter((x) => x !== t.slug),
                    )
                  }
                />
                <bdi>{cleanName(t.name)}</bdi>
              </label>
            ))}
          </div>
        </fieldset>
      )}
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.body}>
          Page <span className={ui.hint}>(markdown)</span>
        </label>
        <textarea
          id={ids.body}
          className={ui.input}
          rows={14}
          value={markdown}
          onChange={(e) => setMarkdown(e.target.value)}
        />
      </div>
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.summary}>
          Edit summary <span className={ui.hint}>(optional, public)</span>
        </label>
        <input
          id={ids.summary}
          className={ui.input}
          value={summary}
          maxLength={300}
          onChange={(e) => setSummary(e.target.value)}
        />
      </div>
      {problem && (
        <div className={ui.error} role="alert">
          <p>{problem}</p>
          {moved && (
            <button type="button" className={ui.button} onClick={reload}>
              Reload the page
            </button>
          )}
        </div>
      )}
      {headMoved && !moved && (
        <div className={ui.notice} role="status">
          <p>{HEAD_MOVED_MESSAGE}</p>
          <button type="button" className={ui.button} onClick={reload}>
            Reload the page
          </button>
        </div>
      )}
      <p className={ui.small}>
        Each save is a new public revision in your repository; revisions are never edited, and the
        page's history keeps every one.
      </p>
      <div className={ui.row}>
        <button type="submit" className={ui.primary} disabled={busy}>
          {base === undefined ? "Create page" : "Save page"}
        </button>
      </div>
    </form>
  );
}

export function WikiEditPage() {
  const engine = useEngine();
  const qc = useQueryClient();
  const { forum } = useForumView();
  const { tag } = route.useParams();
  const active = useActiveAccount();
  const view = useQuery({
    queryKey: qk.tag(forum.uri, tag),
    queryFn: () => engine.tag(forum.uri, tag),
    retry: false,
  });
  // a page with no revision (or not a tag) is not a TagView: it is written as a new page
  const notFound = view.isError && isEngineError(view.error) && view.error.code === "not_found";
  // The head the form started from, kept until the author reloads: a newer head arriving while
  // they write never replaces their text, and their save names this one as `base` (O-2).
  const [pinned, setPinned] = useState<{ head: string | undefined; isNew: boolean } | null>(null);
  const [carried, setCarried] = useState<WikiForm | null>(null);
  if (pinned === null && (view.isSuccess || notFound)) {
    setPinned({ head: view.data?.head, isNew: notFound });
  }
  const head = pinned?.head;
  const isNew = pinned?.isNew ?? notFound;
  const record = useQuery({
    queryKey: qk.raw(head ?? ""),
    queryFn: () => engine.raw(head ?? ""),
    enabled: head !== undefined,
  });
  const reload = async (mine: WikiForm): Promise<void> => {
    try {
      const fresh = await qc.fetchQuery({
        queryKey: qk.tag(forum.uri, tag),
        queryFn: () => engine.tag(forum.uri, tag),
        staleTime: 0,
        retry: false,
      });
      setCarried(mine);
      setPinned({ head: fresh.head, isNew: false });
    } catch (e) {
      if (isEngineError(e) && e.code === "not_found") {
        setCarried(mine);
        setPinned({ head: undefined, isNew: true });
      } else {
        toasts.show({ message: errorMessage(e), tone: "error" });
      }
    }
  };
  if (active.data === null) return <p className={ui.notice}>Sign in to edit wiki pages.</p>;
  if (!SLUG_RE.test(tag) || new TextEncoder().encode(tag).length > 64) {
    return <ErrorView error={new Error("Page addresses are lowercase words joined by hyphens.")} />;
  }
  if (pinned === null) {
    if (view.isError && !notFound) {
      return <ErrorView error={view.error} title="This page cannot be edited" />;
    }
    return <Loading />;
  }
  if (active.isPending) return <Loading />;
  if (!isNew && head === undefined) {
    return (
      <ErrorView
        error={new Error("This page has no current revision to edit from.")}
        title="This page cannot be edited"
      />
    );
  }
  if (head !== undefined && record.isPending) return <Loading />;
  if (head !== undefined && record.isError) {
    return <ErrorView error={record.error} title="This page cannot be edited" />;
  }
  const initial = head !== undefined ? wikiFormOf(record.data?.record) : EMPTY_WIKI_FORM;
  if (!initial) {
    return (
      <ErrorView
        error={
          new Error(
            "This page's text is stored in a separate blob that this app cannot load for editing.",
          )
        }
        title="This page cannot be edited"
      />
    );
  }
  // the page's head is no longer the one the form started from (or a page appeared)
  const moved = view.data !== undefined && view.data.head !== head;
  return (
    <section className={ui.page} aria-label={isNew ? "New wiki page" : "Edit wiki page"}>
      <h2 className={ui.sectionTitle}>
        {isNew ? (
          <>
            New page <code>{tag}</code>
          </>
        ) : (
          <>
            Edit <bdi>{cleanName(initial.name)}</bdi>
          </>
        )}
      </h2>
      {isNew && <p className={ui.notice}>{NEW_PAGE_NOTE}</p>}
      {/* the form starts from `initial` once: a reload pins the new head and remounts it */}
      <WikiPageForm
        key={head ?? "new"}
        slug={tag}
        base={head}
        initial={initial}
        moved={moved}
        onReload={(mine) => void reload(mine)}
        carried={carried}
        onDismissCarried={() => setCarried(null)}
      />
    </section>
  );
}
