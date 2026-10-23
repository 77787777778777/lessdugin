// SPDX-License-Identifier: AGPL-3.0-or-later
import { useQueryClient } from "@tanstack/react-query";
import { getRouteApi, Link, Outlet, useNavigate } from "@tanstack/react-router";
import { useEffect } from "react";
import { ActorName } from "../../components/ActorName.tsx";
import { Badges } from "../../components/Badges.tsx";
import { usePublicAck } from "../../components/PublicAck.tsx";
import { ParticipantsProvider } from "../../components/participants.tsx";
import { ErrorView, errorMessage, Loading } from "../../components/Status.tsx";
import { toasts } from "../../components/Toast.tsx";
import { useEngine } from "../../engine/context.tsx";
import { setCurrentForum } from "../../engine/currentForum.ts";
import { useActiveAccount, useForum } from "../../engine/queries.ts";
import { useAnnounceQueued } from "../../engine/queued.ts";
import { useForumSync, useSyncProgress } from "../../engine/sync.ts";
import { usePendingWrite, writeStore } from "../../engine/writes.ts";
import { cleanName } from "../../lib/text.ts";
import ui from "../../styles/ui.module.css";
import styles from "../pages.module.css";
import { ForumContext, isAuthorityRole } from "./forumContext.tsx";

const route = getRouteApi("/f/$owner/$slug");
const NO_ITEMS: never[] = [];

export function ForumLayout() {
  const { owner, slug } = route.useParams();
  const forum = useForum(`#/f/${owner}/${slug}`);
  const engine = useEngine();
  const announceQueued = useAnnounceQueued();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const requestAck = usePublicAck();
  const active = useActiveAccount();
  const uri = forum.data?.forum.uri;
  useEffect(() => {
    if (uri) setCurrentForum(uri);
  }, [uri]);
  useForumSync(uri);
  const progress = useSyncProgress(uri);
  const joining = usePendingWrite(uri ?? "");

  if (forum.isPending) return <Loading label="Opening the forum…" />;
  if (forum.isError)
    return <ErrorView page error={forum.error} title="This forum could not be opened" />;
  const view = forum.data;
  const f = view.forum;
  const params = { owner, slug };
  const authority = isAuthorityRole(view.myRole);

  const signedIn = active.data !== null && active.data !== undefined;
  const join = async (): Promise<void> => {
    if (!signedIn) {
      void navigate({ to: "/login" });
      return;
    }
    if (!(await requestAck("join"))) return;
    try {
      const ticket = await engine.join(f.uri);
      writeStore.track(ticket, f.uri);
      const final = await writeStore.settle(ticket.id);
      if (final.kind === "failed") throw final.error;
      if (final.kind === "queued") {
        announceQueued("Your membership", final.state);
      } else if (final.kind === "done") {
        toasts.show({ message: "Joined (your membership is public)", tone: "success" });
      } else {
        toasts.show({ message: "Join undone" });
      }
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
    }
  };

  return (
    <ForumContext value={view}>
      <ParticipantsProvider forum={view} items={NO_ITEMS}>
        <header className={styles.forumHeader}>
          <h1 className={styles.forumTitle}>
            <bdi>{cleanName(f.name)}</bdi>
          </h1>
          {f.description && <p className={styles.forumDescription}>{f.description}</p>}
          <div className={ui.row}>
            <span className={ui.small}>
              Owner: <ActorName actor={f.owner} />
            </span>
            <span className={ui.chip}>
              {f.membership === "open" ? "Open membership" : "Approval required"}
            </span>
            {view.myRole !== "none" && <span className={ui.chip}>Your role: {view.myRole}</span>}
            {view.myKarma !== undefined && <span className={ui.chip}>Karma {view.myKarma}</span>}
            <Badges completeness={view.completeness} pending={joining} />
            {progress && (
              <span className={ui.small} role="status">
                Syncing {progress.done}/{progress.total}…
              </span>
            )}
            {!f.member &&
              (signedIn ? (
                <button
                  type="button"
                  className={ui.primary}
                  disabled={joining}
                  onClick={() => void join()}
                >
                  Join
                </button>
              ) : (
                <Link to="/login" className={ui.button}>
                  Sign in to join
                </Link>
              ))}
            <button
              type="button"
              className={ui.ghost}
              onClick={() =>
                void (f.followed ? engine.forumUnfollow(f.uri) : engine.forumFollow(f.uri)).then(
                  () => qc.invalidateQueries(),
                )
              }
            >
              {f.followed ? "Unfollow" : "Follow"}
            </button>
          </div>
          {view.banners.map((b) => (
            <p key={b.code} className={styles.banner} data-banner={b.code}>
              {b.message}
            </p>
          ))}
          {view.completeness.degraded && (
            <p className={styles.banner}>
              Moderation data unavailable: some items are hidden until it loads.
            </p>
          )}
          <nav className={styles.tabs} aria-label="Forum sections">
            <Link
              to="/f/$owner/$slug"
              params={params}
              className={styles.tab}
              activeProps={{ className: styles.tabActive }}
              activeOptions={{ exact: true, includeSearch: false }}
            >
              Posts
            </Link>
            <Link
              to="/f/$owner/$slug/activity"
              params={params}
              className={styles.tab}
              activeProps={{ className: styles.tabActive }}
            >
              Recent discussion
            </Link>
            <Link
              to="/f/$owner/$slug/new"
              params={params}
              className={styles.tab}
              activeProps={{ className: styles.tabActive }}
            >
              New post
            </Link>
            <Link
              to="/f/$owner/$slug/tags"
              params={params}
              className={styles.tab}
              activeProps={{ className: styles.tabActive }}
            >
              Tags
            </Link>
            <Link
              to="/f/$owner/$slug/seqs"
              params={params}
              className={styles.tab}
              activeProps={{ className: styles.tabActive }}
            >
              Sequences
            </Link>
            <Link
              to="/f/$owner/$slug/members"
              params={params}
              className={styles.tab}
              activeProps={{ className: styles.tabActive }}
            >
              Members
            </Link>
            <Link
              to="/f/$owner/$slug/mod"
              params={params}
              className={styles.tab}
              activeProps={{ className: styles.tabActive }}
            >
              {authority ? "Moderation" : "Mod log"}
            </Link>
            <Link
              to="/f/$owner/$slug/settings"
              params={params}
              className={styles.tab}
              activeProps={{ className: styles.tabActive }}
            >
              About
            </Link>
          </nav>
        </header>
        <Outlet />
      </ParticipantsProvider>
    </ForumContext>
  );
}
