// SPDX-License-Identifier: AGPL-3.0-or-later
// A sequence edit starts from the record, and the author's entry moves name record positions:
// the view leaves out the entries it cannot display, so its indices are not the record's.
import { describe, expect, it } from "vitest";
import {
  entryMove,
  recordChapters,
  recordIndices,
  sequenceDraftOf,
  sequenceFormOf,
} from "./sequences.ts";

describe("sequenceFormOf / sequenceDraftOf", () => {
  const record = {
    forum: "at://did:plc:owner/online.7778777.lessdugin.forum/f",
    title: "Reading list",
    body: { text: "Start here", media: [] },
    chapters: [
      { title: "One", posts: ["at://a/p/1", 7, "at://a/p/hidden"] },
      { body: { text: "No title" }, posts: [] },
    ],
  };

  it("keeps every entry of the record, the ones the view does not show included", () => {
    const form = sequenceFormOf(record);
    expect(form?.chapters.map((c) => c.posts)).toEqual([["at://a/p/1", "at://a/p/hidden"], []]);
    expect(form?.chapters[1]?.markdown).toBe("No title");
    const draft = sequenceDraftOf(
      record.forum,
      form ?? { ...record, markdown: "", media: [], chapters: [] },
    );
    expect(draft).toEqual({
      forum: record.forum,
      title: "Reading list",
      body: { markdown: "Start here", media: [] },
      chapters: [
        { title: "One", posts: ["at://a/p/1", "at://a/p/hidden"] },
        { body: { markdown: "No title", media: [] }, posts: [] },
      ],
    });
  });

  it("leaves an emptied body out, and refuses a spilled one", () => {
    const form = sequenceFormOf(record);
    if (!form) throw new Error("no form");
    expect(sequenceDraftOf(record.forum, { ...form, markdown: "  " }).body).toBeUndefined();
    expect(sequenceFormOf({ ...record, body: { textBlob: { ref: { $link: "b" } } } })).toBeNull();
  });
});

describe("entry positions", () => {
  // the record's chapters, and what the view lists (h1 and h2 are hidden)
  const record = [
    ["p1", "h1", "p2", "p3"],
    ["h2", "q1"],
  ];
  const listed = [["p1", "p2", "p3"], ["q1"]];

  it("maps the view's entries to record indices, a repeated post included", () => {
    expect(recordIndices(record[0] ?? [], listed[0] ?? [])).toEqual([0, 2, 3]);
    expect(recordIndices(["a", "x", "a"], ["a", "a"])).toEqual([0, 2]);
    expect(recordIndices(["a"], ["b"])).toEqual([-1]);
  });

  it("moves past hidden entries within a chapter", () => {
    // p2 (record 2) up before p1 (record 0); p1 down after p2 (record 2, read after removal)
    expect(entryMove(record, listed, 0, 1, "up")).toEqual({
      from: { chapter: 0, index: 2 },
      to: { chapter: 0, index: 0 },
    });
    expect(entryMove(record, listed, 0, 0, "down")).toEqual({
      from: { chapter: 0, index: 0 },
      to: { chapter: 0, index: 2 },
    });
  });

  it("moves across chapters at their ends, and nowhere past the sequence's", () => {
    expect(entryMove(record, listed, 0, 2, "down")).toEqual({
      from: { chapter: 0, index: 3 },
      to: { chapter: 1, index: 0 },
    });
    expect(entryMove(record, listed, 1, 0, "up")).toEqual({
      from: { chapter: 1, index: 1 },
      to: { chapter: 0, index: 4 },
    });
    expect(entryMove(record, listed, 0, 0, "up")).toBeUndefined();
    expect(entryMove(record, listed, 1, 0, "down")).toBeUndefined();
  });

  it("reads the record's chapters", () => {
    expect(recordChapters({ chapters: [{ posts: ["a", 1] }, {}] })).toEqual([["a"], []]);
    expect(recordChapters(null)).toEqual([]);
  });
});
