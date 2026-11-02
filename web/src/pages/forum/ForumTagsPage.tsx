// SPDX-License-Identifier: AGPL-3.0-or-later
import { useQuery } from "@tanstack/react-query";
import { getRouteApi, Link, useNavigate } from "@tanstack/react-router";
import { type FormEvent, useId, useState } from "react";
import { SLUG_RE } from "../../components/ForumForm.tsx";
import { ErrorView, Loading } from "../../components/Status.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
import { cleanName } from "../../lib/text.ts";
import ui from "../../styles/ui.module.css";
import { useForumView } from "./forumContext.tsx";

const route = getRouteApi("/f/$owner/$slug/tags");

/** Starts a wiki page: its address (the tag's slug) opens the page editor, which writes the
 * first revision (or edits the page, if it exists already). */
function NewPage() {
  const navigate = useNavigate();
  const params = route.useParams();
  const ids = { slug: useId(), hint: useId() };
  const [slug, setSlug] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const submit = (e: FormEvent): void => {
    e.preventDefault();
    const s = slug.trim();
    if (!SLUG_RE.test(s) || new TextEncoder().encode(s).length > 64) {
      setProblem("Use lowercase letters and digits, words joined by hyphens.");
      return;
    }
    void navigate({ to: "/f/$owner/$slug/t/$tag/edit", params: { ...params, tag: s } });
  };
  return (
    <form className={ui.field} onSubmit={submit} noValidate aria-label="New wiki page">
      <label className={ui.label} htmlFor={ids.slug}>
        New page address
      </label>
      <div className={ui.row}>
        <input
          id={ids.slug}
          className={ui.input}
          value={slug}
          maxLength={64}
          autoCapitalize="none"
          spellCheck={false}
          aria-describedby={ids.hint}
          onChange={(e) => setSlug(e.target.value.toLowerCase())}
        />
        <button type="submit" className={ui.button}>
          Start page
        </button>
      </div>
      <span id={ids.hint} className={ui.hint}>
        The tag's slug, for example <code>decision-theory</code>.
      </span>
      {problem && (
        <p className={ui.error} role="alert">
          {problem}
        </p>
      )}
    </form>
  );
}

export function ForumTagsPage() {
  const engine = useEngine();
  const { forum } = useForumView();
  const params = route.useParams();
  const tags = useQuery({ queryKey: qk.tags(forum.uri), queryFn: () => engine.tags(forum.uri) });
  if (tags.isPending) return <Loading />;
  if (tags.isError) return <ErrorView error={tags.error} />;
  return (
    <section className={ui.page} aria-label="Tags">
      <h2 className={ui.sectionTitle}>Tags</h2>
      <NewPage />
      <div className={ui.tableScroll}>
        <table className={ui.table}>
          <thead>
            <tr>
              <th scope="col">Tag</th>
              <th scope="col">Posts</th>
              <th scope="col">Description</th>
            </tr>
          </thead>
          <tbody>
            {tags.data.map((t) => (
              <tr key={t.slug}>
                <td>
                  <Link to="/f/$owner/$slug/t/$tag" params={{ ...params, tag: t.slug }}>
                    <bdi>{cleanName(t.name)}</bdi>
                  </Link>
                  {t.unmergedEdits > 0 && (
                    <span className={ui.muted}> ({t.unmergedEdits} unmerged edits)</span>
                  )}
                </td>
                <td>{t.postCount}</td>
                <td className={ui.muted}>{t.description}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
