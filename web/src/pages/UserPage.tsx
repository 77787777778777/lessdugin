// SPDX-License-Identifier: AGPL-3.0-or-later
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { useState } from "react";
import { ActorName } from "../components/ActorName.tsx";
import { PostListItem } from "../components/PostListItem.tsx";
import { RecordLink } from "../components/RecordLink.tsx";
import { RenderedHtml } from "../components/RenderedHtml.tsx";
import { SafeLink } from "../components/SafeLink.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { useEngine } from "../engine/context.tsx";
import { useCurrentForum } from "../engine/currentForum.ts";
import { qk, useActiveAccount } from "../engine/queries.ts";
import { cleanName } from "../lib/text.ts";
import { shortAge } from "../lib/time.ts";
import ui from "../styles/ui.module.css";
import styles from "./pages.module.css";

const route = getRouteApi("/u/$actor");

/** Keyed by the actor: going from /u/A to /u/B must not carry A's local state (the mute
 * override) over to B. */
export function UserPage() {
  const { actor } = route.useParams();
  return <UserProfile key={actor} actor={actor} />;
}

function UserProfile({ actor }: { actor: string }) {
  const engine = useEngine();
  const qc = useQueryClient();
  const forum = useCurrentForum() ?? undefined;
  const q = useQuery({ queryKey: qk.user(actor, forum), queryFn: () => engine.user(forum, actor) });
  const active = useActiveAccount();
  const subs = useQuery({ queryKey: qk.subscriptions(), queryFn: () => engine.subscriptions() });
  const [mutedOverride, setMutedOverride] = useState<boolean | null>(null);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorView page error={q.error} title="Unknown user" />;
  const u = q.data;
  const isMe = active.data?.did === u.actor.did;
  const subscribed =
    subs.data?.some((s) => s.subscription.kind === "user" && s.subscription.did === u.actor.did) ??
    false;
  // Mutes are local lens state; the lens shows up as `collapsed: muted` on the user's items.
  const mutedByLens = [...u.recentPosts, ...u.recentComments].some(
    (x) => x.state.kind === "collapsed" && x.state.reason === "muted",
  );
  const muted = mutedOverride ?? mutedByLens;
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>
        <ActorName actor={u.actor} link={false} />
      </h1>
      <dl className={styles.kv}>
        <dt>Handle</dt>
        <dd>
          {u.actor.handle ? (
            `@${u.actor.handle}`
          ) : (
            <span className={ui.muted}>not verified ({u.actor.handleState})</span>
          )}
        </dd>
        <dt>DID</dt>
        <dd className={ui.mono}>{u.actor.did}</dd>
        <dt>Status</dt>
        <dd>{u.status}</dd>
        {u.karma && (
          <>
            <dt>Karma</dt>
            <dd>
              {u.karma.k}{" "}
              <span className={ui.muted}>
                (K1 {u.karma.k1}, KP {u.karma.kp})
              </span>
            </dd>
          </>
        )}
      </dl>
      {!isMe && (
        <p className={ui.row}>
          <button
            type="button"
            className={ui.button}
            aria-pressed={subscribed}
            onClick={() =>
              void engine
                .setSubscription({ kind: "user", did: u.actor.did }, !subscribed)
                .then(() => qc.invalidateQueries({ queryKey: qk.subscriptions() }))
            }
          >
            {subscribed ? "Unsubscribe" : "Subscribe to posts"}
          </button>
          <button
            type="button"
            className={ui.ghost}
            aria-pressed={muted}
            onClick={() =>
              void engine.mute(u.actor.did, !muted).then(() => setMutedOverride(!muted))
            }
          >
            {muted ? "Unmute" : "Mute (local)"}
          </button>
        </p>
      )}
      {u.bio && <RenderedHtml html={u.bio.html} />}
      {u.links.length > 0 && (
        <ul className={ui.list}>
          {u.links.map((l) => (
            <li key={l.uri}>
              <SafeLink raw={l.uri}>
                {l.label ? <bdi>{cleanName(l.label, 100)}</bdi> : l.uri}
              </SafeLink>
            </li>
          ))}
        </ul>
      )}
      {u.pinned.length > 0 && (
        <>
          <h2 className={ui.sectionTitle}>Pinned</h2>
          {u.pinned.map((p) => (
            <PostListItem key={p.uri} post={p} />
          ))}
        </>
      )}
      <h2 className={ui.sectionTitle}>Posts</h2>
      {u.recentPosts.length === 0 && <p className={ui.muted}>No posts.</p>}
      {u.recentPosts.map((p) => (
        <PostListItem key={p.uri} post={p} />
      ))}
      <h2 className={ui.sectionTitle}>Comments</h2>
      {u.recentComments.length === 0 && <p className={ui.muted}>No comments.</p>}
      {u.recentComments.map((c) => (
        <div key={c.uri} className={styles.miniComment}>
          <div className={styles.miniMeta}>
            <RecordLink uri={c.uri}>{shortAge(c.tRankMs)}</RecordLink>
            <span>{c.score.score} karma</span>
          </div>
          <RenderedHtml html={c.body.html} className="lfm-compact" />
        </div>
      ))}
    </div>
  );
}
