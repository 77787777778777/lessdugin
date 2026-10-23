// SPDX-License-Identifier: AGPL-3.0-or-later
import type { ForumDraft, ForumSummary } from "@lessdugin/engine/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { ActorName } from "../components/ActorName.tsx";
import { EMPTY_FORUM_DRAFT, ForumForm } from "../components/ForumForm.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { toasts } from "../components/Toast.tsx";
import { useEngine } from "../engine/context.tsx";
import { qk, useActiveAccount, useFollowed } from "../engine/queries.ts";
import { useAnnounceQueued } from "../engine/queued.ts";
import { writeStore } from "../engine/writes.ts";
import { cleanName } from "../lib/text.ts";
import { parseAtUri } from "../lib/uri.ts";
import ui from "../styles/ui.module.css";
import styles from "./pages.module.css";

function ForumCard({ forum }: { forum: ForumSummary }) {
  const engine = useEngine();
  const qc = useQueryClient();
  const p = parseAtUri(forum.uri);
  if (!p) return null;
  return (
    <article className={styles.forumCard} aria-label={cleanName(forum.name)}>
      <h2 className={styles.forumCardTitle}>
        <Link to="/f/$owner/$slug" params={{ owner: p.did, slug: p.rkey }}>
          <bdi>{cleanName(forum.alias ?? forum.name)}</bdi>
        </Link>
      </h2>
      {forum.description && <p className={ui.muted}>{forum.description}</p>}
      <div className={ui.row}>
        <span className={ui.small}>
          Owner: <ActorName actor={forum.owner} />
        </span>
        <span className={ui.chip}>
          {forum.membership === "open" ? "Open membership" : "Approval required"}
        </span>
        {forum.member && <span className={ui.chip}>Member</span>}
        <button
          type="button"
          className={ui.ghost}
          onClick={() =>
            void (
              forum.followed ? engine.forumUnfollow(forum.uri) : engine.forumFollow(forum.uri)
            ).then(() => qc.invalidateQueries({ queryKey: qk.followed() }))
          }
        >
          {forum.followed ? "Unfollow" : "Follow"}
        </button>
      </div>
    </article>
  );
}

/** "Create a forum": the declaration and the owner's membership, in the viewer's repository. */
function CreateForum() {
  const engine = useEngine();
  const announceQueued = useAnnounceQueued();
  const navigate = useNavigate();
  const active = useActiveAccount();
  const [open, setOpen] = useState(false);
  if (!active.data) {
    return active.isPending ? null : (
      <p className={ui.small}>
        <Link to="/login">Sign in</Link> to create a forum.
      </p>
    );
  }
  if (!open) {
    return (
      <p>
        <button type="button" className={ui.button} onClick={() => setOpen(true)}>
          Create a forum
        </button>
      </p>
    );
  }
  const create = async (draft: ForumDraft): Promise<boolean> => {
    const ticket = await engine.forumCreate(draft);
    writeStore.track(ticket);
    const final = await writeStore.settle(ticket.id);
    if (final.kind === "failed") throw final.error;
    if (final.kind === "queued") {
      announceQueued("Your new forum", final.state);
      setOpen(false);
      return true;
    }
    if (final.kind !== "done") return false;
    toasts.show({ message: "Forum created (its declaration is public)", tone: "success" });
    const p = parseAtUri(final.uri);
    if (p) void navigate({ to: "/f/$owner/$slug", params: { owner: p.did, slug: p.rkey } });
    return true;
  };
  return (
    <section aria-labelledby="create-forum" className={ui.card}>
      <h2 id="create-forum" className={ui.sectionTitle}>
        Create a forum
      </h2>
      <ForumForm
        initial={EMPTY_FORUM_DRAFT}
        creating
        submitLabel="Create the forum"
        onSubmit={create}
        onCancel={() => setOpen(false)}
      />
    </section>
  );
}

export function HomePage() {
  const engine = useEngine();
  const followed = useFollowed();
  const discover = useQuery({ queryKey: qk.discover(""), queryFn: () => engine.forumDiscover("") });
  const others = (discover.data ?? []).filter((f) => !f.followed);
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Your forums</h1>
      <p className={ui.muted}>
        Forums you follow. Posts, comments and votes live in their authors' own repositories; this
        app folds them into one view.
      </p>
      {followed.isPending && <Loading />}
      {followed.isError && <ErrorView error={followed.error} />}
      {followed.data && followed.data.length === 0 && <p>You do not follow any forum yet.</p>}
      <div className={styles.cardList}>
        {followed.data?.map((f) => (
          <ForumCard key={f.uri} forum={f} />
        ))}
      </div>
      <CreateForum />
      {others.length > 0 && (
        <>
          <h2 className={ui.sectionTitle}>Discover</h2>
          <div className={styles.cardList}>
            {others.map((f) => (
              <ForumCard key={f.uri} forum={f} />
            ))}
          </div>
        </>
      )}
    </div>
  );
}
