// SPDX-License-Identifier: AGPL-3.0-or-later
// Leaving the app for a sign-in page: the authorization server's URL from the OAuth client, or
// the one `lessdugin web` returns from `/bridge/login`. The only place allowed to navigate the
// tab through `location` (scripts/check-no-raw-href.mjs). The URL must be https, or http on a
// loopback host (the devnet; `lessdugin web` serves on 127.0.0.1), never `javascript:` and
// friends, and it carries no credentials.

const LOOPBACK = new Set(["localhost", "127.0.0.1", "[::1]"]);

/** Whether `url` may be the target of a sign-in redirect. */
export function isAuthRedirect(url: URL): boolean {
  if (url.username !== "" || url.password !== "") return false;
  if (url.protocol === "https:") return true;
  return url.protocol === "http:" && LOOPBACK.has(url.hostname);
}

/** Sends the tab to the sign-in page at `url`, or throws when it is not one. */
export function goToSignIn(url: URL, win: Pick<Window, "location"> = window): void {
  if (!isAuthRedirect(url)) {
    throw new Error(`refusing to leave for ${url.protocol} sign-in page`);
  }
  win.location.assign(url.href);
}
