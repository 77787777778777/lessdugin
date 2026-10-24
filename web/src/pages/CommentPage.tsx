// SPDX-License-Identifier: AGPL-3.0-or-later
// A comment permalink: the comment and its replies, re-rooted (also the target of
// "continue this thread →").

import { isMissingRoot } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import type { ThreadContext } from "../components/CommentItem.tsx";
import { CommentTree } from "../components/CommentTree.tsx";
import { postTitle } from "../components/PostMeta.tsx";
import { ParticipantsProvider } from "../components/participants.tsx";
import { RecordLink } from "../components/RecordLink.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { subtree } from "../components/threadRows.ts";
import { useEngine } from "../engine/context.tsx";
import { qk, useActiveAccount, useForum, useParsedRef } from "../engine/queries.ts";
import { votingMode } from "../lib/policy.ts";
import ui from "../styles/ui.module.css";
import { isAuthorityRole } from "./forum/forumContext.tsx";

const route = getRouteApi("/c/$author/$rkey");

/** The thread root the comment record names, used only when the engine gives no
 * `RawView.root` (the thread it placed the comment in). The record is author-controlled, so
 * this only chooses which thread to ask the engine for: the comment is shown only if the
 * engine's own thread for that root lists it (`subtree` below), otherwise the page says it is
 * not shown. */
function rootOf(record: unknown): string | null {
  if (typeof record !== "object" || record === null) return null;
  const root = (record as { root?: { uri?: unknown } }).root;
  return typeof root?.uri === "string" ? root.uri : null;
}

function CommentThread({ uri }: { uri: string }) {
  const engine = useEngine();
  const raw = useQuery({ queryKey: qk.raw(uri), queryFn: () => engine.raw(uri) });
  const rootUri = raw.data ? (raw.data.root ?? rootOf(raw.data.record)) : null;
  const thread = useQuery({
    queryKey: qk.thread(rootUri ?? "", "magic", false),
    queryFn: () => engine.getThread(rootUri ?? "", { sort: "magic", unreadOnly: false }),
    enabled: rootUri !== null,
  });
  const active = useActiveAccount();
  const root = thread.data?.post;
  const post = root && !isMissingRoot(root) ? root.post : null;
  const forum = useForum(post?.forum);
  if (raw.isPending || (rootUri !== null && thread.isPending)) return <Loading />;
  if (raw.isError) return <ErrorView page error={raw.error} title="Unknown comment" />;
  if (thread.isError) return <ErrorView page error={thread.error} />;
  if (!rootUri || !thread.data)
    return <ErrorView page error={new Error("This comment has no thread.")} />;
  const comments = subtree(thread.data.comments, uri);
  const first = comments[0];
  const role = forum.data?.myRole ?? "none";
  const viewer = forum.data?.viewer;
  const ctx: ThreadContext = {
    postUri: rootUri,
    forum: post?.forum ?? "",
    votingMode: votingMode(forum.data?.policy),
    locked: post?.locked ?? false,
    myDid: active.data?.did ?? null,
    isAuthority: viewer !== undefined ? viewer.moderate : isAuthorityRole(role),
    canCurate: viewer !== undefined ? viewer.curate : role === "owner" || role === "admin",
    caps: viewer,
  };
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Comment thread</h1>
      <p className={ui.row}>
        <span className={ui.muted}>On</span>
        <RecordLink uri={rootUri}>
          {post ? <bdi>{postTitle(post)}</bdi> : "the original post"}
        </RecordLink>
        {first?.parent && (
          <RecordLink uri={first.parent} className={ui.small}>
            ↑ Parent comment
          </RecordLink>
        )}
      </p>
      {comments.length === 0 ? (
        <p className={ui.notice}>
          This comment is not shown in its thread (it may be hidden or deleted).
        </p>
      ) : (
        // Mentions resolve eagerly only for forum participants (lfm-1.md §5.4).
        <ParticipantsProvider
          forum={forum.data}
          items={post ? [post, ...thread.data.comments] : thread.data.comments}
        >
          <CommentTree comments={comments} ctx={ctx} baseDepth={first?.depth ?? 1} />
        </ParticipantsProvider>
      )}
    </div>
  );
}

export function CommentPage() {
  const { author, rkey } = route.useParams();
  const ref = useParsedRef(`#/c/${author}/${rkey}`);
  if (ref.isPending) return <Loading />;
  if (ref.isError) return <ErrorView page error={ref.error} title="Unknown comment" />;
  if (ref.data.kind !== "comment")
    return <ErrorView page error={new Error("This address is not a comment.")} />;
  return <CommentThread key={ref.data.uri} uri={ref.data.uri} />;
}
