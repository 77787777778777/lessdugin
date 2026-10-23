// SPDX-License-Identifier: AGPL-3.0-or-later
// Routes (synthesis §6.12), on hash history so the static build works from any host and path.

import type { PostFilter, PostSort } from "@lessdugin/engine/types";
import { POST_FILTERS, POST_SORTS } from "@lessdugin/engine/types";
import { createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { AppShell } from "./components/AppShell.tsx";
import { createAppHashHistory } from "./lib/hashHistory.ts";
import { parseAtUri } from "./lib/uri.ts";
import { AtPage } from "./pages/AtPage.tsx";
import { BookmarksPage } from "./pages/BookmarksPage.tsx";
import { CommentPage } from "./pages/CommentPage.tsx";
import { DraftsPage } from "./pages/DraftsPage.tsx";
import { ForumActivityPage } from "./pages/forum/ForumActivityPage.tsx";
import { ForumLayout } from "./pages/forum/ForumLayout.tsx";
import { ForumMembersPage } from "./pages/forum/ForumMembersPage.tsx";
import { ForumModPage } from "./pages/forum/ForumModPage.tsx";
import { ForumNewPostPage } from "./pages/forum/ForumNewPostPage.tsx";
import { ForumPostsPage } from "./pages/forum/ForumPostsPage.tsx";
import { ForumSequencesPage } from "./pages/forum/ForumSequencesPage.tsx";
import { ForumSettingsPage } from "./pages/forum/ForumSettingsPage.tsx";
import { ForumTagsPage } from "./pages/forum/ForumTagsPage.tsx";
import { TagPage } from "./pages/forum/TagPage.tsx";
import { HomePage } from "./pages/HomePage.tsx";
import { LoginPage } from "./pages/LoginPage.tsx";
import { NotFoundPage } from "./pages/NotFoundPage.tsx";
import { NotificationsPage } from "./pages/NotificationsPage.tsx";
import { PostPage } from "./pages/PostPage.tsx";
import { SearchPage } from "./pages/SearchPage.tsx";
import { SequencePage } from "./pages/SequencePage.tsx";
import { SettingsPage } from "./pages/SettingsPage.tsx";
import { UserPage } from "./pages/UserPage.tsx";

export interface ForumSearch {
  sort?: PostSort;
  filter?: PostFilter;
  tag?: string;
}

function str(v: unknown): string | undefined {
  return typeof v === "string" && v !== "" ? v : undefined;
}

export function validateForumSearch(search: Record<string, unknown>): ForumSearch {
  const out: ForumSearch = {};
  const sort = str(search.sort);
  const filter = str(search.filter);
  const tag = str(search.tag);
  if (sort && (POST_SORTS as readonly string[]).includes(sort)) out.sort = sort as PostSort;
  if (filter && (POST_FILTERS as readonly string[]).includes(filter))
    out.filter = filter as PostFilter;
  if (tag && /^[a-z0-9]+(-[a-z0-9]+)*$/.test(tag)) out.tag = tag;
  return out;
}

const rootRoute = createRootRoute({ component: AppShell, notFoundComponent: NotFoundPage });

const homeRoute = createRoute({ getParentRoute: () => rootRoute, path: "/", component: HomePage });

const forumRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/f/$owner/$slug",
  component: ForumLayout,
  validateSearch: validateForumSearch,
});

const forumIndexRoute = createRoute({
  getParentRoute: () => forumRoute,
  path: "/",
  component: ForumPostsPage,
});
const forumActivityRoute = createRoute({
  getParentRoute: () => forumRoute,
  path: "activity",
  component: ForumActivityPage,
});
const forumNewRoute = createRoute({
  getParentRoute: () => forumRoute,
  path: "new",
  component: ForumNewPostPage,
  validateSearch: (s: Record<string, unknown>): { draft?: string; edit?: string } => {
    const draft = str(s.draft);
    const edit = str(s.edit);
    return {
      ...(draft ? { draft } : {}),
      ...(edit && parseAtUri(edit) ? { edit } : {}),
    };
  },
});
const forumTagsRoute = createRoute({
  getParentRoute: () => forumRoute,
  path: "tags",
  component: ForumTagsPage,
});
const forumTagRoute = createRoute({
  getParentRoute: () => forumRoute,
  path: "t/$tag",
  component: TagPage,
});
const forumSeqsRoute = createRoute({
  getParentRoute: () => forumRoute,
  path: "seqs",
  component: ForumSequencesPage,
});
const forumModRoute = createRoute({
  getParentRoute: () => forumRoute,
  path: "mod",
  component: ForumModPage,
});
const forumMembersRoute = createRoute({
  getParentRoute: () => forumRoute,
  path: "members",
  component: ForumMembersPage,
});
const forumSettingsRoute = createRoute({
  getParentRoute: () => forumRoute,
  path: "settings",
  component: ForumSettingsPage,
});

const postRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/p/$author/$rkey",
  component: PostPage,
});
const commentRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/c/$author/$rkey",
  component: CommentPage,
});
const sequenceRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/s/$author/$rkey",
  component: SequencePage,
});
const userRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/u/$actor",
  component: UserPage,
});
const atRoute = createRoute({ getParentRoute: () => rootRoute, path: "/at/$", component: AtPage });
const notificationsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/notifications",
  component: NotificationsPage,
});
const draftsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/drafts",
  component: DraftsPage,
});
const bookmarksRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/bookmarks",
  component: BookmarksPage,
});
const searchRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/search",
  component: SearchPage,
  validateSearch: (s: Record<string, unknown>): { q?: string } => {
    const q = typeof s.q === "string" ? s.q : typeof s.q === "number" ? String(s.q) : undefined;
    return q !== undefined && q !== "" ? { q } : {};
  },
});
const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: SettingsPage,
});
const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  component: LoginPage,
  // `error`: the engine error code `lessdugin web` sends a failed sign-in back with.
  validateSearch: (s: Record<string, unknown>): { error?: string } => {
    const e = str(s.error);
    return e && /^[a-z_]{1,40}$/.test(e) ? { error: e } : {};
  },
});

export const routeTree = rootRoute.addChildren([
  homeRoute,
  forumRoute.addChildren([
    forumIndexRoute,
    forumActivityRoute,
    forumNewRoute,
    forumTagsRoute,
    forumTagRoute,
    forumSeqsRoute,
    forumModRoute,
    forumMembersRoute,
    forumSettingsRoute,
  ]),
  postRoute,
  commentRoute,
  sequenceRoute,
  userRoute,
  atRoute,
  notificationsRoute,
  draftsRoute,
  bookmarksRoute,
  searchRoute,
  settingsRoute,
  loginRoute,
]);

export function createAppRouter() {
  return createRouter({
    routeTree,
    history: createAppHashHistory(),
    defaultPreload: false,
    scrollRestoration: true,
    pathParamsAllowedCharacters: [":", "@"],
  });
}

export type AppRouter = ReturnType<typeof createAppRouter>;

declare module "@tanstack/react-router" {
  interface Register {
    router: AppRouter;
  }
}
