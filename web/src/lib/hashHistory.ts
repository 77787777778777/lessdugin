// SPDX-License-Identifier: AGPL-3.0-or-later
// Hash history whose routes (path and query) live entirely in the fragment.
//
// TanStack's createHashHistory appends the page's real query string to the route
// (`/?mock#/search?q=x` would parse as `/search?q=x?mock`). The real query string belongs to
// the page (engine selection, `?mock`), not to the route, so this history leaves it out of
// route parsing and keeps it unchanged in every href it creates.

import { createBrowserHistory, parseHref, type RouterHistory } from "@tanstack/history";

/** The route href (`/path?query#hash`) carried by a URL fragment. */
export function routeHrefFromHash(hash: string): string {
  const fragment = hash.startsWith("#") ? hash.slice(1) : hash;
  const [path = "", ...rest] = fragment.split("#");
  const route = path === "" ? "/" : path;
  return rest.length > 0 ? `${route}#${rest.join("#")}` : route;
}

export function createAppHashHistory(win: Window = window): RouterHistory {
  return createBrowserHistory({
    window: win,
    parseLocation: () => parseHref(routeHrefFromHash(win.location.hash), win.history.state),
    createHref: (href) => `${win.location.pathname}${win.location.search}#${href}`,
  });
}
