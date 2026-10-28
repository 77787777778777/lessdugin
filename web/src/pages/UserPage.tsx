// SPDX-License-Identifier: AGPL-3.0-or-later
// A user's profile. Until the engine has the `user` operation (M5) it shows the identity
// alone, with subscribing and muting, which need nothing more.
import { type ActorView, isEngineError } from "@lessdugin/engine/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { useState } from "react";
import { ActorName } from "../components/ActorName.tsx";
import { PostListItem } from "../components/PostListItem.tsx";
import { RecordLink } from "../components/RecordLink.tsx";
import { RenderedHtml } from "../components/RenderedHtml.tsx";
import { SafeLink } from "../components/SafeLink.tsx";
import { ErrorView, errorMessage, Loading } from "../components/Status.tsx";
import { toasts } from "../components/Toast.tsx";
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

/** Subscribe and mute (local lens state) for another user. */
function UserActions({ did, mutedByLens }: { did: string; mutedByLens: boolean | undefined }) {
  const engine = useEngine();
  const qc = useQueryClient();
  const subs = useQuery({ queryKey: qk.subscriptions(), queryFn: () => engine.subscriptions() });
  // the device's mute list, when the profile's items cannot tell (no `user` op before M5)
  const mutes = useQuery({
    queryKey: qk.mutes(),
    queryFn: () => engine.mutes(),
    enabled: mutedByLens === undefined,
  });
  const [mutedOverride, setMutedOverride] = useState<boolean | null>(null);
  const subscribed =
    subs.data?.some((s) => s.subscription.kind === "user" && s.subscription.did === did) ?? false;
  const muted = mutedOverride ?? mutedByLens ?? mutes.data?.some((a) => a.did === did) ?? false;
  return (
    <p className={ui.row}>
      <button
        type="button"
        className={ui.button}
        aria-pressed={subscribed}
        onClick={() =>
          void engine
            .setSubscription({ kind: "user", did }, !subscribed)
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
          void engine
            .mute(did, !muted)
            .then(() => {
              setMutedOverride(!muted);
              return qc.invalidateQueries({ queryKey: qk.mutes() });
            })
            .catch((e: unknown) => toasts.show({ message: errorMessage(e), tone: "error" }))
        }
      >
        {muted ? "Unmute" : "Mute (local)"}
      </button>
    </p>
  );
}

/** The identity alone, while the engine has no `user` operation (M5): subscribing and muting
 * need nothing more. */
function IdentityProfile({ actor }: { actor: string }) {
  const engine = useEngine();
  const active = useActiveAccount();
  const who = useQuery({
    queryKey: qk.actor(actor),
    queryFn: () => engine.resolveActor(actor),
    staleTime: 5 * 60_000,
  });
  if (who.isPending) return <Loading />;
  if (who.isError) return <ErrorView page error={who.error} title="Unknown user" />;
  const a = who.data;
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>
        <ActorName actor={a} link={false} />
      </h1>
      <IdentityList actor={a} />
      {active.data?.did !== a.did && <UserActions did={a.did} mutedByLens={undefined} />}
      <p className={ui.muted}>This client cannot show profiles, posts or karma yet.</p>
    </div>
  );
}

function IdentityList({ actor, status }: { actor: ActorView; status?: string | undefined }) {
  return (
    <dl className={styles.kv}>
      <dt>Handle</dt>
      <dd>
        {actor.handle ? (
          `@${actor.handle}`
        ) : (
          <span className={ui.muted}>not verified ({actor.handleState})</span>
        )}
      </dd>
      <dt>DID</dt>
      <dd className={ui.mono}>{actor.did}</dd>
      {status !== undefined && (
        <>
          <dt>Status</dt>
          <dd>{status}</dd>
        </>
      )}
    </dl>
  );
}

export function UserProfile({ actor }: { actor: string }) {
  const engine = useEngine();
  const forum = useCurrentForum() ?? undefined;
  const q = useQuery({ queryKey: qk.user(actor, forum), queryFn: () => engine.user(forum, actor) });
  const active = useActiveAccount();
  if (q.isPending) return <Loading />;
  if (q.isError && isEngineError(q.error) && q.error.code === "unsupported") {
    return <IdentityProfile actor={actor} />;
  }
  if (q.isError) return <ErrorView page error={q.error} title="Unknown user" />;
  const u = q.data;
  const isMe = active.data?.did === u.actor.did;
  // Mutes are local lens state; the lens shows up as `collapsed: muted` on the user's items.
  const mutedByLens = [...u.recentPosts, ...u.recentComments].some(
    (x) => x.state.kind === "collapsed" && x.state.reason === "muted",
  );
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>
        <ActorName actor={u.actor} link={false} />
      </h1>
      <IdentityList actor={u.actor} status={u.status} />
      {u.karma && (
        <dl className={styles.kv}>
          <dt>Karma</dt>
          <dd>
            {u.karma.k}{" "}
            <span className={ui.muted}>
              (K1 {u.karma.k1}, KP {u.karma.kp})
            </span>
          </dd>
        </dl>
      )}
      {!isMe && <UserActions did={u.actor.did} mutedByLens={mutedByLens} />}
      {u.bio && <RenderedHtml html={u.bio.html} media={u.bio.media} />}
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
          <RenderedHtml html={c.body.html} media={c.body.media} className="lfm-compact" />
        </div>
      ))}
    </div>
  );
}
