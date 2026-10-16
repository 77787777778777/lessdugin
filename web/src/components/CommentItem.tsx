// SPDX-License-Identifier: AGPL-3.0-or-later
// One comment row: header (collapse toggle, author, time, votes, flags, badges), the rendered
// body with reacts, and actions (reply, link, moderate). Tombstones, missing placeholders and
// collapsed comments render their reason instead of the body (social.md §10.2).

import type { CommentNode, HiddenReason } from "@lessdugin/engine/types";
import { isOrphanAnchor } from "@lessdugin/engine/types";
import { useNavigate } from "@tanstack/react-router";
import { type FocusEvent, type KeyboardEvent, useRef } from "react";
import { useEngine } from "../engine/context.tsx";
import { useCommentDraft } from "../engine/drafts.ts";
import { usePendingWrite, writeStore } from "../engine/writes.ts";
import { cx } from "../lib/cx.ts";
import type { VotingMode } from "../lib/policy.ts";
import { quoteFromSelection } from "../lib/quote.ts";
import { fullDate, shortAge } from "../lib/time.ts";
import ui from "../styles/ui.module.css";
import { ActorName, actorLabel } from "./ActorName.tsx";
import { Badges } from "./Badges.tsx";
import styles from "./CommentTree.module.css";
import { LazyComposer } from "./LazyComposer.tsx";
import { ModMenu } from "./ModMenu.tsx";
import { OwnCommentActions } from "./OwnCommentActions.tsx";
import { COLLAPSE_LABEL } from "./PostMeta.tsx";
import { ReactBar } from "./Reacts.tsx";
import { RecordLink } from "./RecordLink.tsx";
import { RenderedHtml, type RenderedHtmlHandle } from "./RenderedHtml.tsx";
import { Loading } from "./Status.tsx";
import { toasts } from "./Toast.tsx";
import { type CommentRow, THREAD_AUTHOR_EXPANDED_MAX } from "./threadRows.ts";
import { VoteControls } from "./VoteControls.tsx";

export interface ThreadContext {
  postUri: string;
  forum: string;
  votingMode: VotingMode;
  locked: boolean;
  myDid: string | null;
  isAuthority: boolean;
  canCurate: boolean;
}

/** Why a hidden item is shown as a tombstone (social.md §10.1–10.2). */
export function hiddenReasonText(reason: HiddenReason): string {
  switch (reason.kind) {
    case "removed": {
      const base = reason.spam ? "Removed by a moderator as spam" : "Removed by a moderator";
      return reason.reason ? `${base}: ${reason.reason}` : base;
    }
    case "banned":
      return "Hidden: the author is banned from this forum";
    case "restricted":
      return "Hidden: the author is restricted from this kind of content";
    case "authorModerated":
      return "Hidden by the post's author";
    case "locked":
      return "Hidden: the thread is locked";
    case "newUser":
      return "Hidden: from a new user";
    case "notParticipant":
      return "Hidden: the author is not a participant of this forum";
    case "moderationUnavailable":
      return "Hidden: moderation data is unavailable";
  }
}

function versionNote(node: CommentNode): string | null {
  if (!node.version) return null;
  if ("revision" in node.version) return `on revision ${node.version.revision}`;
  return node.version.unavailable ? "on an unavailable version" : null;
}

const INDENT_CLASS = ["", "indent1", "indent2", "indent3", "indent4", "indent5", "indent6"];

function hiddenRepliesText(n: number): string {
  return `${n} ${n === 1 ? "reply" : "replies"} hidden.`;
}

/** The reply composer, resuming a saved draft for this parent if there is one. */
function ReplyBox({
  ctx,
  node,
  onDone,
  onSubmit,
}: {
  ctx: ThreadContext;
  node: CommentNode;
  onDone: () => void;
  onSubmit: (markdown: string) => Promise<boolean>;
}) {
  const { ready, draft } = useCommentDraft(ctx.postUri, node.uri);
  if (!ready) return <Loading label="Loading the editor…" />;
  return (
    <LazyComposer
      compact
      autoFocus
      label={`Reply to ${actorLabel(node.author)}`}
      submitLabel="Publish reply"
      placeholder="Write a reply…"
      initial={draft?.markdown ?? ""}
      draftId={draft?.id}
      makeDraft={(markdown, id) => ({
        kind: "comment",
        ...(id !== undefined ? { id } : {}),
        comment: { root: ctx.postUri, parent: node.uri, body: { markdown, media: [] } },
      })}
      onSubmit={onSubmit}
      onCancel={onDone}
    />
  );
}

export function CommentItem({
  row,
  index,
  ctx,
  focused,
  replying,
  onToggle,
  onFocusRow,
  onReply,
  onShowAuthor,
}: {
  row: CommentRow;
  index: number;
  ctx: ThreadContext;
  focused: boolean;
  replying: boolean;
  onToggle: (uri: string) => void;
  /** The row with this comment uri received focus. */
  onFocusRow: (uri: string) => void;
  onReply: (uri: string | null) => void;
  /** Expands every comment of this author hidden by the per-author limit. */
  onShowAuthor?: ((did: string) => void) | undefined;
}) {
  const engine = useEngine();
  const navigate = useNavigate();
  const bodyRef = useRef<RenderedHtmlHandle>(null);
  const { node } = row;
  const pending = usePendingWrite(node.uri);
  const indentClass = styles[INDENT_CLASS[row.indent] ?? "indent6"];
  const onFocus = (e: FocusEvent<HTMLElement>): void => {
    if (e.target === e.currentTarget) onFocusRow(node.uri);
  };
  /** Enter on the focused row expands or collapses it (rows with a collapse control only). */
  const onKeyDown = (e: KeyboardEvent<HTMLElement>): void => {
    if (e.key === "Enter" && e.target === e.currentTarget) {
      e.preventDefault();
      onToggle(node.uri);
    }
  };
  const placeholderKeyDown = (e: KeyboardEvent<HTMLElement>): void => {
    if (row.collapsed) onKeyDown(e);
  };

  const placeholder = (label: string, className: string | undefined, text: string) => (
    <article
      data-row-index={index}
      data-comment-uri={node.uri}
      tabIndex={-1}
      onFocus={onFocus}
      onKeyDown={placeholderKeyDown}
      className={cx(styles.comment, className, indentClass, focused && styles.focused)}
      aria-label={label}
    >
      <p className={styles.reason}>{text}</p>
      {row.collapsed && (
        <p className={styles.reason}>
          <button
            type="button"
            className={styles.toggle}
            aria-expanded={false}
            onClick={() => onToggle(node.uri)}
          >
            [+]
          </button>{" "}
          {hiddenRepliesText(row.hiddenReplies)}
        </p>
      )}
    </article>
  );

  if (node.missing) {
    return placeholder(
      "Missing comment",
      styles.placeholder,
      "[missing comment] The comment these replies answer is not available.",
    );
  }

  if (node.state.kind === "tombstone" || node.state.kind === "hidden") {
    return placeholder(
      "Removed comment",
      styles.tombstone,
      `[${hiddenReasonText(node.state.reason)}]`,
    );
  }

  const mine = ctx.myDid !== null && node.author.did === ctx.myDid;
  const signedIn = ctx.myDid !== null;
  const version = versionNote(node);
  const collapsedReason = node.state.kind === "collapsed" ? node.state.reason : null;
  const quoteOrphan = node.quoteAnchor !== undefined && isOrphanAnchor(node.quoteAnchor);

  const submitReply = async (markdown: string): Promise<boolean> => {
    const ticket = await engine.comment({
      root: ctx.postUri,
      parent: node.uri,
      body: { markdown, media: [] },
    });
    writeStore.track(ticket);
    const final = await writeStore.wait(ticket.id);
    if (final.kind === "failed") throw final.error;
    if (final.kind !== "done") return false;
    toasts.show({ message: "Reply published", tone: "success" });
    onReply(null);
    return true;
  };

  return (
    <article
      data-row-index={index}
      data-comment-uri={node.uri}
      tabIndex={-1}
      onFocus={onFocus}
      onKeyDown={onKeyDown}
      className={cx(
        styles.comment,
        indentClass,
        node.unread && styles.unread,
        focused && styles.focused,
        mine && styles.mine,
      )}
      aria-label={`Comment by ${actorLabel(node.author)}${node.unread ? " (new)" : ""}`}
    >
      <header className={styles.header}>
        <button
          type="button"
          className={styles.toggle}
          aria-expanded={!row.collapsed}
          aria-label={row.collapsed ? "Expand comment" : "Collapse comment"}
          onClick={() => onToggle(node.uri)}
        >
          {row.collapsed ? "[+]" : "[−]"}
        </button>
        <ActorName actor={node.author} />
        {node.moderatorHat && <span className={styles.hat}>Moderator</span>}
        <RecordLink uri={node.uri} className={styles.time}>
          <time dateTime={new Date(node.tRankMs).toISOString()} title={fullDate(node.createdAtMs)}>
            {shortAge(node.tRankMs)}
          </time>
        </RecordLink>
        {!row.collapsed && (
          <VoteControls
            subject={node.uri}
            score={node.score}
            mode={ctx.votingMode}
            ownItem={mine}
            signedIn={signedIn}
          />
        )}
        {node.pinned && <span className={styles.flag}>📌 Pinned</span>}
        {node.unread && <span className={styles.newFlag}>New</span>}
        {version && <span className={styles.note}>{version}</span>}
        {node.orphaned && <span className={styles.note}>re-attached reply</span>}
        {node.retracted && <span className={styles.note}>retracted</span>}
        <Badges badges={node.badges} pending={mine && pending} />
      </header>
      {row.collapsed ? (
        <p className={styles.reason}>
          {collapsedReason
            ? `Collapsed: ${COLLAPSE_LABEL[collapsedReason]}. `
            : row.authorOverflow !== undefined
              ? `Collapsed: more than ${THREAD_AUTHOR_EXPANDED_MAX} comments by this author. `
              : "Collapsed. "}
          {row.hiddenReplies > 0 && hiddenRepliesText(row.hiddenReplies)}
          {row.authorOverflow !== undefined && onShowAuthor && (
            <>
              {" "}
              <button
                type="button"
                className={ui.link}
                onClick={() => onShowAuthor(node.author.did)}
              >
                Show {row.authorOverflow} more by <bdi>{actorLabel(node.author)}</bdi>
              </button>
            </>
          )}
        </p>
      ) : (
        <>
          {node.quote && (
            <blockquote className={cx(styles.quote, quoteOrphan && styles.orphanQuote)}>
              {quoteOrphan ? "quoted text no longer present" : node.quote.exact}
            </blockquote>
          )}
          <div className={cx(node.retracted && styles.retracted)}>
            <RenderedHtml html={node.body.html} className="lfm-compact" ref={bodyRef} />
          </div>
          {ctx.votingMode === "reacts" && (
            <ReactBar
              subject={node.uri}
              reacts={node.reacts}
              ownItem={mine}
              signedIn={signedIn}
              getQuote={() => quoteFromSelection(bodyRef.current?.root() ?? null, node.body.text)}
            />
          )}
          <footer className={styles.footer}>
            {signedIn ? (
              <button
                type="button"
                className={ui.link}
                disabled={ctx.locked && !ctx.isAuthority}
                title={ctx.locked && !ctx.isAuthority ? "Comments are locked" : undefined}
                aria-expanded={replying}
                onClick={() => onReply(replying ? null : node.uri)}
              >
                Reply
              </button>
            ) : (
              <button
                type="button"
                className={ui.link}
                onClick={() => void navigate({ to: "/login" })}
              >
                Sign in to reply
              </button>
            )}
            {mine && <OwnCommentActions node={node} postUri={ctx.postUri} />}
            {ctx.isAuthority && (
              <ModMenu
                forum={ctx.forum}
                canCurate={ctx.canCurate}
                subject={{
                  kind: "comment",
                  uri: node.uri,
                  cid: node.cid,
                  author: node.author.did,
                  pinned: node.pinned,
                }}
              />
            )}
          </footer>
          {replying && signedIn && (
            <div className={styles.replyBox}>
              <ReplyBox ctx={ctx} node={node} onDone={() => onReply(null)} onSubmit={submitReply} />
            </div>
          )}
        </>
      )}
    </article>
  );
}
