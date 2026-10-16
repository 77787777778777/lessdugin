// SPDX-License-Identifier: AGPL-3.0-or-later
import type { ForumView } from "@lessdugin/engine/types";
import { createContext, use } from "react";
import { parseAtUri } from "../../lib/uri.ts";

export const ForumContext = createContext<ForumView | null>(null);

export function useForumView(): ForumView {
  const f = use(ForumContext);
  if (!f) throw new Error("useForumView outside a forum route");
  return f;
}

export function forumParams(uri: string): { owner: string; slug: string } {
  const p = parseAtUri(uri);
  return { owner: p?.did ?? "", slug: p?.rkey ?? "" };
}

export function isAuthorityRole(role: ForumView["myRole"]): boolean {
  return role === "owner" || role === "admin" || role === "moderator";
}
