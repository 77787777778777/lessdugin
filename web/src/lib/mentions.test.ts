// The mention token walk (docs/plan.md M4: "mention autocomplete"). The interesting cases are
// the ones that must NOT open a completion: an email address, a path segment, a token the caret
// has already left.
import { describe, expect, it } from "vitest";

import { applyMention, type MentionCandidate, mentionAt, rankMentions } from "./mentions.ts";

describe("mentionAt", () => {
  it("finds the token the caret is inside", () => {
    expect(mentionAt("hi @ali", 7)).toEqual({ start: 3, query: "ali" });
    expect(mentionAt("@a", 2)).toEqual({ start: 0, query: "a" });
  });

  it("opens right after the at-sign", () => {
    expect(mentionAt("say @", 5)).toEqual({ start: 4, query: "" });
  });

  it("does not treat an email or a path as a mention", () => {
    // "someone@example" has a non-space before the at-sign
    expect(mentionAt("someone@exa", 11)).toBeNull();
    expect(mentionAt("see /u/@x", 9)).toBeNull();
  });

  it("stops at whitespace and brackets", () => {
    expect(mentionAt("@ali bob", 8)).toBeNull();
    expect(mentionAt("(@ali)", 3)).toEqual({ start: 1, query: "a" });
  });

  it("refuses characters a handle cannot contain", () => {
    expect(mentionAt("@al!ce", 6)).toBeNull();
  });
});

describe("rankMentions", () => {
  const c = (handle: string, label?: string): MentionCandidate => ({
    did: `did:plc:${handle}`,
    handle,
    ...(label ? { label } : {}),
  });

  it("puts an exact handle first, then prefixes, then label matches", () => {
    const got = rankMentions("ali", [c("bob"), c("alice"), c("ali"), c("alina", "Ali N")]);
    expect(got.map((x) => x.handle)).toEqual(["ali", "alice", "alina"]);
  });

  it("matches on the display name when the handle does not", () => {
    const got = rankMentions("zed", [c("alice", "Zed"), c("bob")]);
    expect(got.map((x) => x.handle)).toEqual(["alice"]);
  });

  it("keeps one entry per handle and honours the limit", () => {
    const dupes = [c("alice"), { did: "did:plc:other", handle: "alice" }];
    expect(rankMentions("al", dupes)).toHaveLength(1);
    const many = Array.from({ length: 20 }, (_, i) => c(`al${i}`));
    expect(rankMentions("al", many, 5)).toHaveLength(5);
  });
});

describe("applyMention", () => {
  it("inserts the handle with a trailing space and moves the caret past it", () => {
    const out = applyMention("hi @ali there", 3, 7, "alice.example.com");
    expect(out.text).toBe("hi @alice.example.com  there");
    // start + "@" + handle + " " is where the next character goes
    expect(out.caret).toBe(3 + 1 + "alice.example.com".length + 1);
  });
});
