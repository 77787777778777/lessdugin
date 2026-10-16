// SPDX-License-Identifier: AGPL-3.0-or-later
import type { DraftEntry } from "@lessdugin/engine/types";
import { describe, expect, it } from "vitest";
import { latestCommentDraft } from "./drafts.ts";

const ROOT = "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.post/3kabc";
const PARENT = "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.comment/3kdef";

const entry = (id: string, savedAtMs: number, markdown: string, parent?: string): DraftEntry => ({
  id,
  savedAtMs,
  draft: {
    kind: "comment",
    id,
    comment: { root: ROOT, ...(parent ? { parent } : {}), body: { markdown, media: [] } },
  },
});

describe("resuming comment drafts", () => {
  it("picks the latest draft for the same place only", () => {
    const drafts = [
      entry("a", 1, "old top-level"),
      entry("b", 3, "reply", PARENT),
      entry("c", 2, "new top-level"),
    ];
    expect(latestCommentDraft(drafts, ROOT, undefined)).toEqual({
      id: "c",
      markdown: "new top-level",
    });
    expect(latestCommentDraft(drafts, ROOT, PARENT)).toEqual({ id: "b", markdown: "reply" });
    expect(latestCommentDraft(drafts, PARENT, undefined)).toBeNull();
  });
});
