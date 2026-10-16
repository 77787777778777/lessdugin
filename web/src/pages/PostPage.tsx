// SPDX-License-Identifier: AGPL-3.0-or-later
// A post: title, byline, ToC sidebar (from body.toc), the rendered body with footnotes as
// sidenotes on wide screens, votes and reacts, history, sequence navigation, and the thread.

import type { CommentNode, CommentSort, PostView } from "@lessdugin/engine/types";
import { COMMENT_SORTS, isMissingRoot } from "@lessdugin/engine/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { type ReactNode, useEffect, useId, useMemo, useRef, useState } from "react";
import { ActorName } from "../components/ActorName.tsx";
import { Badges } from "../components/Badges.tsx";
import type { ThreadContext } from "../components/CommentItem.tsx";
import { hiddenReasonText } from "../components/CommentItem.tsx";
import { CommentTree } from "../components/CommentTree.tsx";
import { Dialog } from "../components/Dialog.tsx";
import { LazyComposer } from "../components/LazyComposer.tsx";
import { ModMenu } from "../components/ModMenu.tsx";
import {
  COLLAPSE_LABEL,
  KindBadge,
  postTitle,
  TagChips,
  TierBadge,
} from "../components/PostMeta.tsx";
import { ReactBar } from "../components/Reacts.tsx";
import { RecordLink } from "../components/RecordLink.tsx";
import { RenderedHtml, type RenderedHtmlHandle } from "../components/RenderedHtml.tsx";
import { SafeLink } from "../components/SafeLink.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { toasts } from "../components/Toast.tsx";
import { Toc } from "../components/Toc.tsx";
import { VoteControls } from "../components/VoteControls.tsx";
import { useEngine } from "../engine/context.tsx";
import { qk, useActiveAccount, useForum, useParsedRef } from "../engine/queries.ts";
import { writeStore } from "../engine/writes.ts";
import { votingMode } from "../lib/policy.ts";
import { quoteFromSelection } from "../lib/quote.ts";
import { fullDate, shortAge } from "../lib/time.ts";
import ui from "../styles/ui.module.css";
import { isAuthorityRole } from "./forum/forumContext.tsx";
import styles from "./pages.module.css";

const route = getRouteApi("/p/$author/$rkey");

const SORT_LABEL: Record<CommentSort, string> = {
  magic: "Magic",
  top: "Top",
  new: "New",
  old: "Old",
  latestReply: "Latest reply",
};

function HistoryButton({
  uri,
  revisions,
  redacted,
}: {
  uri: string;
  revisions: number;
  redacted: boolean;
}) {
  const engine = useEngine();
  const [open, setOpen] = useState(false);
  const history = useQuery({
    queryKey: qk.history(uri),
    queryFn: () => engine.history(uri),
    enabled: open,
  });
  if (revisions === 0) return null;
  return (
    <>
      <button type="button" className={ui.link} onClick={() => setOpen(true)}>
        History ({revisions} {revisions === 1 ? "revision" : "revisions"})
      </button>
      <Dialog open={open} onClose={() => setOpen(false)} title="Version history">
        {redacted && <p className={ui.notice}>The author redacted part of this history.</p>}
        {history.isPending && <Loading />}
        {history.data && (
          <ol>
            {history.data.versions.map((v) => (
              <li key={v.n}>
                Version {v.n} · {fullDate(v.createdAtMs)}
                {v.summary && ` · “${v.summary}”`}
                {!v.available && <span className={ui.muted}> (history redacted)</span>}
                <div className={ui.mono}>{v.cid}</div>
              </li>
            ))}
          </ol>
        )}
        <button type="button" className={ui.button} onClick={() => setOpen(false)}>
          Close
        </button>
      </Dialog>
    </>
  );
}

function LocalToggles({ uri }: { uri: string }) {
  const engine = useEngine();
  const qc = useQueryClient();
  const bookmarks = useQuery({ queryKey: qk.bookmarks(), queryFn: () => engine.bookmarks() });
  const subs = useQuery({ queryKey: qk.subscriptions(), queryFn: () => engine.subscriptions() });
  const bookmarked = bookmarks.data?.some((b) => b.uri === uri) ?? false;
  const subscribed =
    subs.data?.some((s) => s.subscription.kind === "post" && s.subscription.uri === uri) ?? false;
  return (
    <span className={ui.row}>
      <button
        type="button"
        className={ui.ghost}
        aria-pressed={bookmarked}
        onClick={() =>
          void engine
            .bookmark(uri, !bookmarked)
            .then(() => qc.invalidateQueries({ queryKey: qk.bookmarks() }))
        }
      >
        {bookmarked ? "★ Bookmarked" : "☆ Bookmark"}
      </button>
      <button
        type="button"
        className={ui.ghost}
        aria-pressed={subscribed}
        onClick={() =>
          void engine
            .setSubscription({ kind: "post", uri }, !subscribed)
            .then(() => qc.invalidateQueries({ queryKey: qk.subscriptions() }))
        }
      >
        {subscribed ? "Subscribed to comments" : "Subscribe to comments"}
      </button>
    </span>
  );
}

function PostBody({
  view,
  ctx,
  myDid,
  children,
}: {
  view: PostView | null;
  ctx: ThreadContext;
  myDid: string | null;
  /** The thread, rendered in the article column below the post. */
  children: ReactNode;
}) {
  if (!view) {
    return (
      <div className={styles.postLayout}>
        <div className={styles.tocColumn} />
        <div className={styles.articleColumn}>
          <p className={styles.hiddenPost}>[The post of this thread is not available.]</p>
          {children}
        </div>
      </div>
    );
  }
  return (
    <PostBodyView view={view} ctx={ctx} myDid={myDid}>
      {children}
    </PostBodyView>
  );
}

function PostBodyView({
  view,
  ctx,
  myDid,
  children,
}: {
  view: PostView;
  ctx: ThreadContext;
  myDid: string | null;
  children: ReactNode;
}) {
  const bodyRef = useRef<RenderedHtmlHandle>(null);
  const titleId = useId();
  const { post, body } = view;
  const own = myDid !== null && post.author.did === myDid;
  const hidden = post.state.kind === "hidden" || post.state.kind === "tombstone";
  return (
    <div className={styles.postLayout}>
      <aside className={styles.tocColumn} aria-label="Contents">
        {body.toc.length > 1 && (
          <Toc entries={body.toc} onSelect={(id) => bodyRef.current?.scrollToAnchor(id)} />
        )}
      </aside>
      <article className={styles.articleColumn} aria-labelledby={titleId}>
        <header>
          <div className={ui.row}>
            <KindBadge kind={post.kind} />
            <TagChips forum={post.forum} tags={post.tags} />
          </div>
          <h1 className={styles.postTitle} id={titleId}>
            <bdi>{postTitle(post)}</bdi>
          </h1>
          <div className={styles.byline}>
            <ActorName actor={post.author} showIdent />
            <time
              dateTime={new Date(post.tRankMs).toISOString()}
              title={fullDate(post.createdAtMs)}
            >
              {shortAge(post.tRankMs)}
            </time>
            {body.readMinutes > 0 && <span>{body.readMinutes} min read</span>}
            <TierBadge tier={post.tier} />
            {post.pinned && <span>📌 Pinned</span>}
            {post.locked && <span>🔒 Comments locked</span>}
            {post.state.kind === "collapsed" && (
              <span>Collapsed: {COLLAPSE_LABEL[post.state.reason]}</span>
            )}
            <Badges badges={post.badges} />
            <HistoryButton
              uri={post.uri}
              revisions={view.history.revisions}
              redacted={view.history.redacted}
            />
          </div>
        </header>
        {post.link && (
          <div className={styles.linkCard}>
            <p className={styles.linkCardTitle}>
              <SafeLink raw={post.link.uri}>{post.link.title ?? post.link.uri}</SafeLink>
            </p>
            {post.link.description && <p className={ui.muted}>{post.link.description}</p>}
          </div>
        )}
        {hidden ? (
          <p className={styles.hiddenPost}>
            [
            {post.state.kind === "hidden" || post.state.kind === "tombstone"
              ? hiddenReasonText(post.state.reason)
              : ""}
            ]
          </p>
        ) : (
          <RenderedHtml html={body.html} sidenotes ref={bodyRef} />
        )}
        {view.sequences.map((s) => (
          <nav key={s.sequence} className={styles.seqNav} aria-label={`Sequence: ${s.title}`}>
            {s.prev ? <RecordLink uri={s.prev}>← Previous</RecordLink> : <span />}
            <RecordLink uri={s.sequence}>
              <bdi>{s.title}</bdi>
            </RecordLink>
            {s.next ? <RecordLink uri={s.next}>Next →</RecordLink> : <span />}
          </nav>
        ))}
        <div className={styles.postFooter}>
          <VoteControls
            subject={post.uri}
            score={post.score}
            mode={ctx.votingMode}
            size="post"
            ownItem={own}
            signedIn={myDid !== null}
          />
          {ctx.votingMode === "reacts" && (
            <ReactBar
              subject={post.uri}
              reacts={view.reacts}
              ownItem={own}
              signedIn={myDid !== null}
              getQuote={() => quoteFromSelection(bodyRef.current?.root() ?? null, body.text)}
            />
          )}
          <LocalToggles uri={post.uri} />
          {ctx.isAuthority && (
            <ModMenu
              forum={post.forum}
              canCurate={ctx.canCurate}
              subject={{
                kind: "post",
                uri: post.uri,
                cid: post.cid,
                author: post.author.did,
                pinned: post.pinned,
                locked: post.locked,
                tier: post.tier,
              }}
            />
          )}
        </div>
      </article>
      <div className={styles.threadColumn}>{children}</div>
    </div>
  );
}

export function PostThread({ uri }: { uri: string }) {
  const engine = useEngine();
  const [sort, setSort] = useState<CommentSort>("magic");
  const [unreadOnly, setUnreadOnly] = useState(false);
  const headingId = useId();
  const thread = useQuery({
    queryKey: qk.thread(uri, sort, unreadOnly),
    queryFn: () => engine.getThread(uri, { sort, unreadOnly }),
  });
  const active = useActiveAccount();
  const root = thread.data?.post;
  const postView = root && !isMissingRoot(root) ? root : null;
  const forumUri = postView?.post.forum;
  const forum = useForum(forumUri);
  const loaded = thread.data !== undefined;

  // Keep this visit's unread highlights after `markRead` re-queries the thread.
  const unreadAtOpen = useRef<Set<string> | null>(null);
  if (thread.data && unreadAtOpen.current === null) {
    unreadAtOpen.current = new Set(thread.data.comments.filter((c) => c.unread).map((c) => c.uri));
  }
  useEffect(() => {
    if (!loaded) return;
    const t = setTimeout(() => void engine.markRead(uri).catch(() => {}), 1500);
    return () => clearTimeout(t);
  }, [loaded, engine, uri]);

  const comments: CommentNode[] = useMemo(() => {
    const seen = unreadAtOpen.current;
    return (thread.data?.comments ?? []).map((c) =>
      !c.unread && seen?.has(c.uri) ? { ...c, unread: true } : c,
    );
  }, [thread.data]);

  if (thread.isPending) return <Loading label="Loading the post…" />;
  if (thread.isError)
    return <ErrorView error={thread.error} title="This post could not be loaded" />;

  const myDid = active.data?.did ?? null;
  const role = forum.data?.myRole ?? "none";
  const ctx: ThreadContext = {
    postUri: uri,
    forum: forumUri ?? "",
    votingMode: votingMode(forum.data?.policy),
    locked: postView?.post.locked ?? false,
    myDid,
    isAuthority: isAuthorityRole(role),
    canCurate: role === "owner" || role === "admin",
  };
  const canComment = myDid !== null && (!ctx.locked || ctx.isAuthority);
  const count = postView?.post.commentCount ?? comments.length;

  const submitComment = async (markdown: string): Promise<boolean> => {
    const ticket = await engine.comment({ root: uri, body: { markdown, media: [] } });
    writeStore.track(ticket);
    const final = await writeStore.wait(ticket.id);
    if (final.kind === "failed") throw final.error;
    if (final.kind !== "done") return false;
    toasts.show({ message: "Comment published", tone: "success" });
    return true;
  };

  return (
    <PostBody view={postView} ctx={ctx} myDid={myDid}>
      <section aria-labelledby={headingId}>
        <div className={styles.commentsHeader}>
          <h2 id={headingId} className={styles.commentsTitle} data-testid="comment-count">
            {count} {count === 1 ? "comment" : "comments"}
          </h2>
          <div className={ui.row}>
            <label className={ui.row}>
              <span className={ui.muted}>Sort</span>
              <select
                className={styles.select}
                value={sort}
                onChange={(e) => setSort(e.target.value as CommentSort)}
              >
                {COMMENT_SORTS.map((s) => (
                  <option key={s} value={s}>
                    {SORT_LABEL[s]}
                  </option>
                ))}
              </select>
            </label>
            <label className={ui.row}>
              <input
                type="checkbox"
                checked={unreadOnly}
                onChange={(e) => setUnreadOnly(e.target.checked)}
              />
              <span>Unread only</span>
            </label>
          </div>
        </div>
        {ctx.locked && !ctx.isAuthority && (
          <p className={ui.notice}>Comments on this post are locked.</p>
        )}
        {canComment && (
          <LazyComposer
            label="New comment"
            submitLabel="Comment"
            placeholder="Write a comment…"
            compact
            makeDraft={(markdown, id) => ({
              kind: "comment",
              ...(id !== undefined ? { id } : {}),
              comment: { root: uri, body: { markdown, media: [] } },
            })}
            onSubmit={submitComment}
          />
        )}
        {myDid === null && <p className={ui.muted}>Sign in to comment.</p>}
        <CommentTree comments={comments} ctx={ctx} />
      </section>
    </PostBody>
  );
}

export function PostPage() {
  const { author, rkey } = route.useParams();
  const ref = useParsedRef(`#/p/${author}/${rkey}`);
  if (ref.isPending) return <Loading />;
  if (ref.isError) return <ErrorView error={ref.error} title="Unknown post" />;
  if (ref.data.kind !== "post")
    return <ErrorView error={new Error("This address is not a post.")} />;
  return <PostThread key={ref.data.uri} uri={ref.data.uri} />;
}
