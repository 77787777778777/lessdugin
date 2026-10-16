// SPDX-License-Identifier: AGPL-3.0-or-later
// Hash history whose routes (path and query) live entirely in the fragment.
//
// TanStack's createHashHistory appends the page's real query string to the route
// (`/?mock#/search?q=x` would parse as `/search?q=x?mock`). The real query string belongs to
// the page, not to the route, so this history leaves it out of route parsing. The hrefs it
// creates keep only the page flags the app knows (`?mock`, honoured only by demo builds): any
// other query parameter (for example an OAuth callback's, which is removed at boot anyway,
// threat-model SPA-8) is never copied into a link.

import { createBrowserHistory, parseHref, type RouterHistory } from "@tanstack/history";

/** The route href (`/path?query#hash`) carried by a URL fragment. */
export function routeHrefFromHash(hash: string): string {
  const fragment = hash.startsWith("#") ? hash.slice(1) : hash;
  const [path = "", ...rest] = fragment.split("#");
  const route = path === "" ? "/" : path;
  return rest.length > 0 ? `${route}#${rest.join("#")}` : route;
}

/** The part of the page's real query string that links keep: only the `mock` flag. */
export function pageQuery(search: string): string {
  return new URLSearchParams(search).has("mock") ? "?mock" : "";
}

export function createAppHashHistory(win: Window = window): RouterHistory {
  return createBrowserHistory({
    window: win,
    parseLocation: () => parseHref(routeHrefFromHash(win.location.hash), win.history.state),
    createHref: (href) => `${win.location.pathname}${pageQuery(win.location.search)}#${href}`,
  });
}
