// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure display logic for threads: turns the engine's pre-order comment list into rows,
// applying collapse state, the indentation cap ("continue thread →", social.md §11) and the
// per-author display limit (social.md §10.3, limits.md `THREAD_AUTHOR_EXPANDED_MAX`): at most 50
// comments per author per thread are expanded by default, in `(t_rank, uri)` order; the rest
// collapse behind "show N more by @x". The limit is presentation only.

import type { CommentNode } from "@lessdugin/engine/types";

export const MAX_INDENT = 6;
export const THREAD_AUTHOR_EXPANDED_MAX = 50;

export interface CommentRow {
  kind: "comment";
  node: CommentNode;
  /** 1-based indentation level, at most `MAX_INDENT`. */
  indent: number;
  collapsed: boolean;
  /** Descendants hidden because this row is collapsed. */
  hiddenReplies: number;
  /** Collapsed by default because its author is over the per-author limit: how many of their
   * comments in this thread are (the "show N more by @x" count). */
  authorOverflow?: number;
}

export interface ContinueRow {
  kind: "continue";
  /** The comment at the cap whose deeper replies continue on its own page. */
  parentUri: string;
  count: number;
  indent: number;
}

export type ThreadRow = CommentRow | ContinueRow;

/** Collapsed by default: items the fold collapsed (newUser, lowScore, muted), and items over the
 * per-author display limit. */
export function collapsedByDefault(
  node: CommentNode,
  overflow: ReadonlySet<string> = new Set(),
): boolean {
  return node.state.kind === "collapsed" || overflow.has(node.uri);
}

/** A comment is collapsed when its default differs from the user's toggle. */
export function isCollapsed(
  node: CommentNode,
  toggled: ReadonlySet<string>,
  overflow: ReadonlySet<string> = new Set(),
): boolean {
  return collapsedByDefault(node, overflow) !== toggled.has(node.uri);
}

/** The comments beyond each author's first `max` in `(t_rank, uri)` order (listed comments
 * only; placeholders never count), except for authors the reader expanded; and how many each
 * author has over the limit. */
export function authorOverflow(
  comments: readonly CommentNode[],
  expandedAuthors: ReadonlySet<string> = new Set(),
  max: number = THREAD_AUTHOR_EXPANDED_MAX,
): { uris: Set<string>; perAuthor: Map<string, number> } {
  const byAuthor = new Map<string, CommentNode[]>();
  for (const c of comments) {
    if (c.missing || (c.state.kind !== "visible" && c.state.kind !== "collapsed")) continue;
    const list = byAuthor.get(c.author.did) ?? [];
    list.push(c);
    byAuthor.set(c.author.did, list);
  }
  const uris = new Set<string>();
  const perAuthor = new Map<string, number>();
  for (const [did, list] of byAuthor) {
    if (list.length <= max || expandedAuthors.has(did)) continue;
    list.sort((a, b) => a.tRankMs - b.tRankMs || (a.uri < b.uri ? -1 : a.uri > b.uri ? 1 : 0));
    for (const c of list.slice(max)) uris.add(c.uri);
    perAuthor.set(did, list.length - max);
  }
  return { uris, perAuthor };
}

/** Number of descendants of `comments[i]` in a pre-order list. */
export function descendantCount(comments: readonly CommentNode[], i: number): number {
  const d = comments[i]?.depth ?? 0;
  let n = 0;
  for (let j = i + 1; j < comments.length && (comments[j]?.depth ?? 0) > d; j++) n++;
  return n;
}

export function buildRows(
  comments: readonly CommentNode[],
  toggled: ReadonlySet<string>,
  opts: {
    maxIndent?: number;
    baseDepth?: number;
    /** Authors whose comments over the per-author limit the reader chose to show. */
    expandedAuthors?: ReadonlySet<string>;
    authorMax?: number;
  } = {},
): ThreadRow[] {
  const maxIndent = opts.maxIndent ?? MAX_INDENT;
  const base = opts.baseDepth ?? 1;
  const overflow = authorOverflow(comments, opts.expandedAuthors, opts.authorMax);
  const rows: ThreadRow[] = [];
  let skipBelow = Number.POSITIVE_INFINITY; // skip rows deeper than this indentation
  const atLevel: string[] = []; // the latest comment uri seen at each indentation level
  for (let i = 0; i < comments.length; i++) {
    const node = comments[i];
    if (!node) continue;
    const indent = node.depth - base + 1;
    if (indent < 1) continue;
    if (indent > skipBelow) continue;
    skipBelow = Number.POSITIVE_INFINITY;
    if (indent > maxIndent) {
      const parentUri = atLevel[maxIndent] ?? node.parent ?? node.uri;
      let count = 1;
      for (
        let j = i + 1;
        j < comments.length && (comments[j]?.depth ?? 0) - base + 1 > maxIndent;
        j++
      )
        count++;
      rows.push({ kind: "continue", parentUri, count, indent: maxIndent });
      skipBelow = maxIndent;
      continue;
    }
    atLevel[indent] = node.uri;
    atLevel.length = indent + 1;
    const collapsed = isCollapsed(node, toggled, overflow.uris);
    const row: CommentRow = {
      kind: "comment",
      node,
      indent,
      collapsed,
      hiddenReplies: collapsed ? descendantCount(comments, i) : 0,
    };
    if (overflow.uris.has(node.uri)) {
      row.authorOverflow = overflow.perAuthor.get(node.author.did) ?? 0;
    }
    rows.push(row);
    if (collapsed) skipBelow = indent;
  }
  return rows;
}

/** The comments of the subtree rooted at `uri` (the root first), for permalink pages. */
export function subtree(comments: readonly CommentNode[], uri: string): CommentNode[] {
  const i = comments.findIndex((c) => c.uri === uri);
  if (i < 0) return [];
  const root = comments[i];
  if (!root) return [];
  const out = [root];
  for (let j = i + 1; j < comments.length; j++) {
    const c = comments[j];
    if (!c || c.depth <= root.depth) break;
    out.push(c);
  }
  return out;
}
