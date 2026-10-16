// SPDX-License-Identifier: AGPL-3.0-or-later
// TanStack Query keys and hooks over the engine, and the mapping from `changed` topics to
// query invalidations (engine-api.md §3.8: "the React app maps Changed topics to TanStack
// Query invalidations"). Keys are prefixed by what a topic invalidates: everything under
// ["forum", uri] for Forum(uri), ["post", uri] for Thread(uri), and so on.

import type { CommentSort, HrefContext, ListPostsQuery, Topic } from "@lessdugin/engine/types";
import { type QueryClient, useQuery } from "@tanstack/react-query";
import { useEngine } from "./context.tsx";

export const qk = {
  forumRef: (ref: string) => ["forumRef", ref] as const,
  posts: (q: ListPostsQuery) =>
    [
      "forum",
      q.forum,
      "posts",
      q.sort,
      q.filter,
      q.tag ?? null,
      q.kind ?? null,
      q.author ?? null,
    ] as const,
  discussion: (forum: string) => ["forum", forum, "discussion"] as const,
  tags: (forum: string) => ["forum", forum, "tags"] as const,
  tag: (forum: string, slug: string) => ["forum", forum, "tag", slug] as const,
  sequences: (forum: string) => ["forum", forum, "sequences"] as const,
  members: (forum: string) => ["forum", forum, "members"] as const,
  modQueue: (forum: string) => ["forum", forum, "modQueue"] as const,
  modLog: (forum: string) => ["forum", forum, "modLog"] as const,
  ref: (input: string) => ["ref", input] as const,
  post: (uri: string) => ["post", uri, "view"] as const,
  thread: (uri: string, sort: CommentSort, unreadOnly: boolean) =>
    ["post", uri, "thread", sort, unreadOnly] as const,
  history: (uri: string) => ["post", uri, "history"] as const,
  raw: (uri: string) => ["raw", uri] as const,
  sequence: (uri: string) => ["sequence", uri] as const,
  user: (actor: string, forum: string | undefined) => ["user", actor, forum ?? null] as const,
  actor: (input: string) => ["actor", input] as const,
  search: (q: string, forum: string | undefined) => ["search", q, forum ?? null] as const,
  notifications: (all: boolean) => ["notifications", all] as const,
  drafts: () => ["drafts"] as const,
  bookmarks: () => ["bookmarks"] as const,
  subscriptions: () => ["subscriptions"] as const,
  accounts: () => ["accounts"] as const,
  activeAccount: () => ["accounts", "active"] as const,
  followed: () => ["followed"] as const,
  discover: (q: string) => ["discover", q] as const,
  settings: () => ["settings"] as const,
  safeHref: (raw: string, ctx: HrefContext) => ["safeHref", ctx, raw] as const,
};

/** The query-key prefixes a topic invalidates. */
export function keysForTopic(topic: Topic): readonly (readonly unknown[])[] {
  switch (topic.kind) {
    case "forum":
      return [
        ["forum", topic.uri],
        ["forumRef"],
        ["followed"],
        ["discover"],
        ["search"],
        ["sequence"],
        ["bookmarks"],
        ["user"],
      ];
    case "thread":
      return [["post", topic.uri], ["raw"], ["bookmarks"], ["search"]];
    case "user":
      return [["user"]];
    case "notifications":
      return [["notifications"], ["subscriptions"]];
    case "drafts":
      return [["drafts"]];
    case "accounts":
      // The viewer changed: "mine" fields, roles and notifications change everywhere.
      return [[]];
    case "outbox":
      return [];
  }
}

export function invalidateTopics(qc: QueryClient, topics: readonly Topic[]): void {
  const seen = new Set<string>();
  for (const t of topics) {
    for (const key of keysForTopic(t)) {
      const id = JSON.stringify(key);
      if (seen.has(id)) continue;
      seen.add(id);
      void qc.invalidateQueries({ queryKey: [...key] });
    }
  }
}

// ------------------------------------------------------------------------------------ hooks

export function useForum(ref: string | undefined) {
  const engine = useEngine();
  return useQuery({
    queryKey: qk.forumRef(ref ?? ""),
    queryFn: () => engine.forumOpen(ref ?? ""),
    enabled: ref !== undefined && ref !== "",
  });
}

export function useParsedRef(input: string | null, forum?: string) {
  const engine = useEngine();
  return useQuery({
    queryKey: qk.ref(input ?? ""),
    queryFn: () => engine.parseRef(input ?? "", forum !== undefined ? { forum } : {}),
    enabled: input !== null,
    staleTime: Number.POSITIVE_INFINITY,
  });
}

export function useActiveAccount() {
  const engine = useEngine();
  return useQuery({ queryKey: qk.activeAccount(), queryFn: () => engine.activeAccount() });
}

export function useSettings() {
  const engine = useEngine();
  return useQuery({ queryKey: qk.settings(), queryFn: () => engine.settings() });
}

export function useFollowed() {
  const engine = useEngine();
  return useQuery({ queryKey: qk.followed(), queryFn: () => engine.followed() });
}

export function useUnreadNotifications() {
  const engine = useEngine();
  return useQuery({
    queryKey: qk.notifications(false),
    queryFn: () => engine.notifications({ all: false, limit: 100 }),
  });
}

export function useSafeHref(raw: string | undefined, ctx: HrefContext) {
  const engine = useEngine();
  return useQuery({
    queryKey: qk.safeHref(raw ?? "", ctx),
    queryFn: () => engine.safeHref(raw ?? "", ctx),
    enabled: raw !== undefined && raw !== "",
    staleTime: Number.POSITIVE_INFINITY,
  });
}
