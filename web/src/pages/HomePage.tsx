// SPDX-License-Identifier: AGPL-3.0-or-later
// The followed forums, creating a forum, and the forum directory (`forum_discover`), which comes
// grouped by owner (sync.md §4.7): each owner's forums are listed under a heading naming the
// owner, so a lookalike forum name shows under its real owner.
import type { ActorView, ForumDraft, ForumSummary } from "@lessdugin/engine/types";
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

/** `heading`: the title's level, below an owner heading in the directory. */
function ForumCard({ forum, heading = "h2" }: { forum: ForumSummary; heading?: "h2" | "h4" }) {
  const engine = useEngine();
  const qc = useQueryClient();
  const p = parseAtUri(forum.uri);
  if (!p) return null;
  const Title = heading;
  return (
    <article className={styles.forumCard} aria-label={cleanName(forum.name)}>
      <Title className={styles.forumCardTitle}>
        <Link to="/f/$owner/$slug" params={{ owner: p.did, slug: p.rkey }}>
          <bdi>{cleanName(forum.alias ?? forum.name)}</bdi>
        </Link>
      </Title>
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

/** The directory's runs of one owner's forums, in the order given. */
export function ownerGroups(
  forums: readonly ForumSummary[],
): { owner: ActorView; forums: ForumSummary[] }[] {
  const groups: { owner: ActorView; forums: ForumSummary[] }[] = [];
  for (const f of forums) {
    const last = groups[groups.length - 1];
    if (last && last.owner.did === f.owner.did) last.forums.push(f);
    else groups.push({ owner: f.owner, forums: [f] });
  }
  return groups;
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
      {discover.isError && (
        <>
          <h2 className={ui.sectionTitle}>Discover</h2>
          <ErrorView error={discover.error} />
        </>
      )}
      {others.length > 0 && (
        <>
          <h2 className={ui.sectionTitle}>Discover</h2>
          {ownerGroups(others).map((g, i) => (
            <div key={`${i}:${g.owner.did}`} className={styles.cardList}>
              <h3 className={styles.ownerHeading} data-owner={g.owner.did}>
                Forums by <ActorName actor={g.owner} />
              </h3>
              {g.forums.map((f) => (
                <ForumCard key={f.uri} forum={f} heading="h4" />
              ))}
            </div>
          ))}
        </>
      )}
    </div>
  );
}
