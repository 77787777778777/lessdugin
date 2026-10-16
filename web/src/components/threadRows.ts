// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure display logic for threads: turns the engine's pre-order comment list into rows,
// applying collapse state and the indentation cap ("continue thread →", social.md §11).

import type { CommentNode } from "@lessdugin/engine/types";

export const MAX_INDENT = 6;

export interface CommentRow {
  kind: "comment";
  node: CommentNode;
  /** 1-based indentation level, at most `MAX_INDENT`. */
  indent: number;
  collapsed: boolean;
  /** Descendants hidden because this row is collapsed. */
  hiddenReplies: number;
}

export interface ContinueRow {
  kind: "continue";
  /** The comment at the cap whose deeper replies continue on its own page. */
  parentUri: string;
  count: number;
  indent: number;
}

export type ThreadRow = CommentRow | ContinueRow;

/** Collapsed by default: items the fold collapsed (newUser, lowScore, muted). */
export function collapsedByDefault(node: CommentNode): boolean {
  return node.state.kind === "collapsed";
}

/** A comment is collapsed when its default differs from the user's toggle. */
export function isCollapsed(node: CommentNode, toggled: ReadonlySet<string>): boolean {
  return collapsedByDefault(node) !== toggled.has(node.uri);
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
  opts: { maxIndent?: number; baseDepth?: number } = {},
): ThreadRow[] {
  const maxIndent = opts.maxIndent ?? MAX_INDENT;
  const base = opts.baseDepth ?? 1;
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
    const collapsed = isCollapsed(node, toggled);
    rows.push({
      kind: "comment",
      node,
      indent,
      collapsed,
      hiddenReplies: collapsed ? descendantCount(comments, i) : 0,
    });
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
