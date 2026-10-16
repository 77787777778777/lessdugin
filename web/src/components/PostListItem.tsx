// SPDX-License-Identifier: AGPL-3.0-or-later
// One row of a post list (ForumMagnum style): score, title with kind badge and flags, byline,
// tier and tags, comment count with unread replies, and the fixed badge column.

import type { PostSummary } from "@lessdugin/engine/types";
import { useId } from "react";
import { cx } from "../lib/cx.ts";
import { fullDate, shortAge } from "../lib/time.ts";
import { ActorName } from "./ActorName.tsx";
import { Badges } from "./Badges.tsx";
import styles from "./PostListItem.module.css";
import { COLLAPSE_LABEL, KindBadge, postTitle, TagChips, TierBadge } from "./PostMeta.tsx";
import { RecordLink } from "./RecordLink.tsx";

export function PostListItem({ post }: { post: PostSummary }) {
  const titleId = useId();
  const collapsed = post.state.kind === "collapsed" ? post.state.reason : null;
  return (
    <article className={cx(styles.item, collapsed && styles.collapsed)} aria-labelledby={titleId}>
      <span className={styles.score} title={`Karma ${post.score.score}`}>
        <span className="visually-hidden">Karma </span>
        {post.score.score}
      </span>
      <div className={styles.main}>
        <h3 className={styles.title} id={titleId}>
          {post.pinned && (
            <span className={styles.flag} title="Pinned">
              <span aria-hidden="true">📌</span>
              <span className="visually-hidden">Pinned: </span>
            </span>
          )}
          <RecordLink uri={post.uri} className={styles.titleLink}>
            <bdi>{postTitle(post)}</bdi>
          </RecordLink>{" "}
          <KindBadge kind={post.kind} />
          {post.locked && (
            <span className={styles.flag} title="Comments are locked">
              <span aria-hidden="true">🔒</span>
              <span className="visually-hidden"> (locked)</span>
            </span>
          )}
        </h3>
        <div className={styles.meta}>
          <ActorName actor={post.author} />
          <time dateTime={new Date(post.tRankMs).toISOString()} title={fullDate(post.createdAtMs)}>
            {shortAge(post.tRankMs)}
          </time>
          <TierBadge tier={post.tier} />
          {post.link && <span className={styles.host}>{post.link.host}</span>}
          <TagChips forum={post.forum} tags={post.tags} />
          {collapsed && (
            <span className={styles.collapsedNote}>Collapsed: {COLLAPSE_LABEL[collapsed]}</span>
          )}
        </div>
      </div>
      <div className={styles.side}>
        <RecordLink uri={post.uri} className={styles.comments}>
          <span aria-hidden="true">💬</span> {post.commentCount}
          <span className="visually-hidden"> comments</span>
          {post.unreadComments !== undefined && post.unreadComments > 0 && (
            <span className={styles.unread}>{post.unreadComments} new</span>
          )}
        </RecordLink>
        <Badges badges={post.badges} />
      </div>
    </article>
  );
}
