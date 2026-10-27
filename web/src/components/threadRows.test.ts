// SPDX-License-Identifier: AGPL-3.0-or-later
import type { CommentNode, ItemState } from "@lessdugin/engine/types";
import { describe, expect, it } from "vitest";
import {
  buildRows,
  collapsedByDefault,
  descendantCount,
  MAX_INDENT,
  subtree,
} from "./threadRows.ts";

function node(
  uri: string,
  depth: number,
  parent?: string,
  state: ItemState = { kind: "visible" },
): CommentNode {
  const n: CommentNode = {
    uri,
    cid: `cid-${uri}`,
    root: "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.post/p",
    depth,
    author: {
      did: "did:plc:abcdefghijklmnopqrstuvwx",
      handleState: "verified",
      shortDid: "did:plc:abcdefgh…",
    },
    body: {
      html: "",
      text: uri,
      toc: [],
      wordCount: 1,
      readMinutes: 1,
      media: [],
      excerpt: uri,
      notices: [],
      rendererVersion: "1",
    },
    createdAtMs: 0,
    tRankMs: 0,
    score: { score: 0, agreement: 0, unweighted: 0, earlierVersionVotes: 0 },
    reacts: [],
    state,
    badges: {},
    retracted: false,
    moderatorHat: false,
    orphaned: false,
    missing: false,
    unread: false,
    pinned: false,
  };
  if (parent !== undefined) n.parent = parent;
  return n;
}

const COLLAPSED: ItemState = { kind: "collapsed", reason: "newUser" };

// a ─ b ─ c        (c collapsed by the fold, with child d)
//   └ e
// f
const thread = [
  node("a", 1),
  node("b", 2, "a"),
  node("c", 3, "b", COLLAPSED),
  node("d", 4, "c"),
  node("e", 2, "a"),
  node("f", 1),
];

const uris = (rows: ReturnType<typeof buildRows>) =>
  rows.map((r) => (r.kind === "comment" ? r.node.uri : `continue:${r.parentUri}`));

describe("thread rows", () => {
  it("expands the viewer's own newcomer comments only (social.md §10.3)", () => {
    const me = "did:plc:abcdefghijklmnopqrstuvwx";
    const c = thread[2] as CommentNode;
    expect(collapsedByDefault(c, new Set(), me)).toBe(false);
    expect(collapsedByDefault(c, new Set(), "did:plc:someoneelseeeeeeeeeeeeee")).toBe(true);
    const lowScore = node("x", 1, undefined, { kind: "collapsed", reason: "lowScore" });
    expect(collapsedByDefault(lowScore, new Set(), me)).toBe(true);
    expect(uris(buildRows(thread, new Set(), { viewer: me }))).toEqual([
      "a",
      "b",
      "c",
      "d",
      "e",
      "f",
    ]);
  });

  it("collapses fold-collapsed comments by default and hides their replies", () => {
    expect(collapsedByDefault(thread[2] as CommentNode)).toBe(true);
    const rows = buildRows(thread, new Set());
    expect(uris(rows)).toEqual(["a", "b", "c", "e", "f"]);
    const c = rows[2];
    expect(c?.kind === "comment" && c.collapsed && c.hiddenReplies).toBe(1);
  });

  it("toggling expands a default-collapsed comment and collapses a visible one", () => {
    expect(uris(buildRows(thread, new Set(["c"])))).toEqual(["a", "b", "c", "d", "e", "f"]);
    const rows = buildRows(thread, new Set(["a"]));
    expect(uris(rows)).toEqual(["a", "f"]);
    const a = rows[0];
    expect(a?.kind === "comment" && a.hiddenReplies).toBe(4);
  });

  it("caps indentation and continues deeper replies on their own page", () => {
    const deep = Array.from({ length: 9 }, (_, i) =>
      node(`n${i + 1}`, i + 1, i > 0 ? `n${i}` : undefined),
    );
    deep.push(node("z", 1));
    const rows = buildRows(deep, new Set());
    expect(uris(rows)).toEqual([
      "n1",
      "n2",
      "n3",
      "n4",
      "n5",
      "n6",
      `continue:n${MAX_INDENT}`,
      "z",
    ]);
    const cont = rows[MAX_INDENT];
    expect(cont?.kind === "continue" && cont.count).toBe(3);
    expect(Math.max(...rows.map((r) => r.indent))).toBe(MAX_INDENT);
  });

  it("re-roots a subtree for permalinks", () => {
    const sub = subtree(thread, "b");
    expect(sub.map((n) => n.uri)).toEqual(["b", "c", "d"]);
    expect(uris(buildRows(sub, new Set(["c"]), { baseDepth: 2 }))).toEqual(["b", "c", "d"]);
    expect(descendantCount(thread, 0)).toBe(4);
  });
});
