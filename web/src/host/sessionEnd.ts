// SPDX-License-Identifier: AGPL-3.0-or-later
// Hosted mode: an OAuth session that ends (its refresh token was refused or revoked, or another
// tab signed out) must read as `authRequired` to the engine, never as a network failure the
// outbox would retry for minutes (robustness review H1). The OAuth library reports an ended
// session by throwing one of its token errors from `fetchHandler`; hosted.ts passes the check
// for those (`isSessionGone`), so this module does not load the library.

import type { PdsSession } from "@lessdugin/engine/client";
import { hostError, oauthSession } from "@lessdugin/engine/client";

export type FetchHandler = (pathname: string, init: RequestInit) => Promise<Response>;

/** The AuthBridge session of `did` over an OAuth `fetchHandler`: a token error rejects with
 * `authRequired` after `onEnded(did)` (which drops the cached session and tells the user). */
export function endAwareSession(
  did: string,
  fetchHandler: FetchHandler,
  isSessionGone: (e: unknown) => boolean,
  onEnded: (did: string) => void,
): PdsSession {
  return oauthSession(did, async (path, init) => {
    try {
      return await fetchHandler(path, init);
    } catch (e) {
      if (!isSessionGone(e)) throw e;
      onEnded(did);
      const why = e instanceof Error && e.message ? ` (${e.message.slice(0, 200)})` : "";
      throw hostError("authRequired", `the session for ${did} has ended${why}: sign in again`);
    }
  });
}
