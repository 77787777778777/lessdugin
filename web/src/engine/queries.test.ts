// SPDX-License-Identifier: AGPL-3.0-or-later
import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { invalidateTopics, keysForTopic, qk } from "./queries.ts";

const FORUM = "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.forum/f";
const POST = "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.post/3kabc";

describe("changed topics → query invalidation (engine-api.md §3.8)", () => {
  it("invalidates post and thread queries when their forum changes", async () => {
    const qc = new QueryClient();
    const keys = [
      qk.post(POST),
      qk.thread(POST, "magic", false),
      qk.posts({ forum: FORUM, sort: "magic", filter: "frontpage", limit: 20 }),
      qk.raw(FORUM),
      qk.drafts(),
    ];
    for (const k of keys) qc.setQueryData([...k], 1);
    invalidateTopics(qc, [{ kind: "forum", uri: FORUM }]);
    const stale = keys.map((k) => qc.getQueryState([...k])?.isInvalidated);
    // Moderation, membership or policy changes can change any item in the forum.
    expect(stale).toEqual([true, true, true, true, false]);
  });

  it("maps each topic kind to its keys", () => {
    expect(keysForTopic({ kind: "thread", uri: POST })).toContainEqual(["post", POST]);
    expect(keysForTopic({ kind: "drafts" })).toEqual([["drafts"]]);
    expect(keysForTopic({ kind: "accounts" })).toEqual([[]]);
    expect(keysForTopic({ kind: "outbox" })).toEqual([]);
  });

  it("refreshes a sequence on its thread topic (its continue-reading position moved)", () => {
    const qc = new QueryClient();
    const SEQ = "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.sequence/s";
    qc.setQueryData([...qk.sequence(SEQ)], 1);
    qc.setQueryData([...qk.sequence(`${SEQ}2`)], 1);
    invalidateTopics(qc, [{ kind: "thread", uri: SEQ }]);
    expect(qc.getQueryState([...qk.sequence(SEQ)])?.isInvalidated).toBe(true);
    expect(qc.getQueryState([...qk.sequence(`${SEQ}2`)])?.isInvalidated).toBe(false);
  });

  it("re-queries notifications on forum and thread topics (they are computed lazily)", () => {
    const qc = new QueryClient();
    for (const topic of [
      { kind: "forum", uri: FORUM },
      { kind: "thread", uri: POST },
    ] as const) {
      qc.setQueryData([...qk.notifications(false)], 1);
      qc.setQueryData([...qk.notifications(true)], 1);
      invalidateTopics(qc, [topic]);
      expect(qc.getQueryState([...qk.notifications(false)])?.isInvalidated).toBe(true);
      expect(qc.getQueryState([...qk.notifications(true)])?.isInvalidated).toBe(true);
    }
  });

  it("keys ref parsing by the forum context (`handle/rkey` refs resolve against it)", () => {
    expect(qk.ref("alice.test/3kabc", FORUM)).not.toEqual(qk.ref("alice.test/3kabc"));
    expect(qk.ref("alice.test/3kabc", FORUM)).toEqual(["ref", "alice.test/3kabc", FORUM]);
  });
});
