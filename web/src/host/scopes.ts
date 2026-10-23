// SPDX-License-Identifier: AGPL-3.0-or-later
// OAuth scope strings (data-model.md §10.3, §10.4; threat-model AUTH-1, AUTH-2): never
// `transition:generic`, never `blob:*/*`. A `.temp` build has no published permission set, so
// it always asks for the enumerated `repo?collection=` form; a release build asks for
// `include:NS.authFull` first and falls back to the enumerated form on `invalid_scope` at PAR,
// before the browser leaves for the authorization server.

import { NS, RECORD_COLLECTIONS } from "@lessdugin/engine/client";

const BLOBS = "blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf";

/** Whether `ns` is a development namespace (`….temp`, data-model.md §1.3). */
export function isTempNamespace(ns: string): boolean {
  return ns.endsWith(".temp");
}

function repoScope(ns: string): string {
  return `repo?${RECORD_COLLECTIONS.map((c) => `collection=${ns}.${c}`).join("&")}`;
}

/** The default scope: `atproto include:NS.authFull` plus the blob scopes. */
export function includeScope(ns: string = NS): string {
  return `atproto include:${ns}.authFull ${BLOBS}`;
}

/** The fallback (and `.temp`, devnet) scope: the 10 collections enumerated. */
export function enumeratedScope(ns: string = NS): string {
  return `atproto ${repoScope(ns)} ${BLOBS}`;
}

/** The scopes to try at PAR, in order. */
export function scopeCandidates(ns: string = NS): string[] {
  return isTempNamespace(ns) ? [enumeratedScope(ns)] : [includeScope(ns), enumeratedScope(ns)];
}

/** `SUPERSET` of data-model.md §10.4: the scope of the client metadata (a `.temp` build omits
 * the `include:` token). */
export function supersetScope(ns: string = NS): string {
  const include = isTempNamespace(ns) ? "" : ` include:${ns}.authFull`;
  return `atproto${include} ${repoScope(ns)} ${BLOBS}`;
}
