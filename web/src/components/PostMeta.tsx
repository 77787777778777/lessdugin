// SPDX-License-Identifier: AGPL-3.0-or-later
import type { CollapseReason, PostKind, PostSummary, Tier } from "@lessdugin/engine/types";
import { Link } from "@tanstack/react-router";
import { cx } from "../lib/cx.ts";
import { cleanName } from "../lib/text.ts";
import { parseAtUri } from "../lib/uri.ts";
import styles from "./PostMeta.module.css";

export const KIND_LABEL: Record<PostKind, string> = {
  article: "Article",
  link: "Link post",
  question: "Question",
  quickTake: "Quick take",
};

export const TIER_LABEL: Record<Tier, string> = {
  personal: "Personal blog",
  frontpage: "Frontpage",
  curated: "Curated",
};

export const COLLAPSE_LABEL: Record<CollapseReason, string> = {
  newUser: "from a new user",
  lowScore: "low score",
  muted: "you muted this author",
};

/** The title of a post for display; quick takes have none, so their excerpt stands in. */
export function postTitle(post: PostSummary): string {
  if (post.kind !== "quickTake" && post.title) return cleanName(post.title, 300);
  const ex = cleanName(post.excerpt, 90);
  return ex === "" ? "Quick take" : ex;
}

export function KindBadge({ kind }: { kind: PostKind }) {
  if (kind === "article") return null;
  return <span className={cx(styles.kind, styles[kind])}>{KIND_LABEL[kind]}</span>;
}

export function TierBadge({ tier }: { tier: Tier }) {
  return (
    <span className={cx(styles.tier, styles[`tier_${tier}`])} title={`Tier: ${TIER_LABEL[tier]}`}>
      {tier === "curated" ? "★ " : ""}
      {TIER_LABEL[tier]}
    </span>
  );
}

export function TagChips({ forum, tags }: { forum: string; tags: string[] }) {
  const f = parseAtUri(forum);
  if (!f || tags.length === 0) return null;
  return (
    <span className={styles.tags}>
      {tags.map((t) => (
        <Link
          key={t}
          to="/f/$owner/$slug/t/$tag"
          params={{ owner: f.did, slug: f.rkey, tag: t }}
          className={styles.tag}
        >
          {t}
        </Link>
      ))}
    </span>
  );
}
