// SPDX-License-Identifier: AGPL-3.0-or-later
// One comment row: header (collapse toggle, author, time, votes, flags, badges), the rendered
// body with reacts, and actions (reply, link, moderate). Tombstones, missing placeholders and
// collapsed comments render their reason instead of the body (social.md §10.2).

import type { CommentNode, HiddenReason } from "@lessdugin/engine/types";
import { useRef } from "react";
import { useEngine } from "../engine/context.tsx";
import { writeStore } from "../engine/writes.ts";
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
import { toasts } from "./Toast.tsx";
import type { CommentRow } from "./threadRows.ts";
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

export function CommentItem({
  row,
  index,
  ctx,
  focused,
  replying,
  onToggle,
  onFocusRow,
  onReply,
}: {
  row: CommentRow;
  index: number;
  ctx: ThreadContext;
  focused: boolean;
  replying: boolean;
  onToggle: (uri: string) => void;
  onFocusRow: (index: number) => void;
  onReply: (uri: string | null) => void;
}) {
  const engine = useEngine();
  const bodyRef = useRef<RenderedHtmlHandle>(null);
  const { node } = row;
  const indentClass = styles[INDENT_CLASS[row.indent] ?? "indent6"];
  const common = {
    "data-row-index": index,
    tabIndex: -1,
    onFocus: (e: React.FocusEvent<HTMLElement>) => {
      if (e.target === e.currentTarget) onFocusRow(index);
    },
    onKeyDown: (e: React.KeyboardEvent<HTMLElement>) => {
      if (e.key === "Enter" && e.target === e.currentTarget) {
        e.preventDefault();
        onToggle(node.uri);
      }
    },
  };

  if (node.missing) {
    return (
      <article
        {...common}
        className={cx(styles.comment, styles.placeholder, indentClass, focused && styles.focused)}
        aria-label="Missing comment"
      >
        <p className={styles.reason}>
          [missing comment] The comment these replies answer is not available.
        </p>
      </article>
    );
  }

  if (node.state.kind === "tombstone" || node.state.kind === "hidden") {
    return (
      <article
        {...common}
        className={cx(styles.comment, styles.tombstone, indentClass, focused && styles.focused)}
        aria-label="Removed comment"
      >
        <p className={styles.reason}>[{hiddenReasonText(node.state.reason)}]</p>
      </article>
    );
  }

  const mine = ctx.myDid !== null && node.author.did === ctx.myDid;
  const version = versionNote(node);
  const collapsedReason = node.state.kind === "collapsed" ? node.state.reason : null;
  const quoteOrphan =
    node.quoteAnchor !== undefined && "orphan" in node.quoteAnchor && node.quoteAnchor.orphan;

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
      {...common}
      className={cx(
        styles.comment,
        indentClass,
        node.unread && styles.unread,
        focused && styles.focused,
        mine && styles.mine,
      )}
      aria-label={`Comment by ${actorLabel(node.author)}${node.unread ? " (new)" : ""}`}
      data-comment-uri={node.uri}
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
            signedIn={ctx.myDid !== null}
          />
        )}
        {node.pinned && <span className={styles.flag}>📌 Pinned</span>}
        {node.unread && <span className={styles.newFlag}>New</span>}
        {version && <span className={styles.note}>{version}</span>}
        {node.orphaned && <span className={styles.note}>re-attached reply</span>}
        {node.retracted && <span className={styles.note}>retracted</span>}
        <Badges badges={node.badges} />
      </header>
      {row.collapsed ? (
        <p className={styles.reason}>
          {collapsedReason ? `Collapsed: ${COLLAPSE_LABEL[collapsedReason]}. ` : "Collapsed. "}
          {row.hiddenReplies > 0 &&
            `${row.hiddenReplies} ${row.hiddenReplies === 1 ? "reply" : "replies"} hidden.`}
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
              signedIn={ctx.myDid !== null}
              getQuote={() => quoteFromSelection(bodyRef.current?.root() ?? null, node.body.text)}
            />
          )}
          <footer className={styles.footer}>
            <button
              type="button"
              className={ui.link}
              disabled={ctx.locked && !ctx.isAuthority}
              title={ctx.locked && !ctx.isAuthority ? "Comments are locked" : undefined}
              onClick={() => onReply(replying ? null : node.uri)}
            >
              Reply
            </button>
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
          {replying && (
            <div className={styles.replyBox}>
              <LazyComposer
                compact
                autoFocus
                label={`Reply to ${actorLabel(node.author)}`}
                submitLabel="Publish reply"
                placeholder="Write a reply…"
                makeDraft={(markdown, id) => ({
                  kind: "comment",
                  ...(id !== undefined ? { id } : {}),
                  comment: { root: ctx.postUri, parent: node.uri, body: { markdown, media: [] } },
                })}
                onSubmit={submitReply}
                onCancel={() => onReply(null)}
              />
            </div>
          )}
        </>
      )}
    </article>
  );
}
