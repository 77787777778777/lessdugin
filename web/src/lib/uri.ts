// SPDX-License-Identifier: AGPL-3.0-or-later
// Helpers over canonical at-uris (data-model.md §2.3). They only split strings that came from
// the engine; anything that came from content is re-parsed by the engine's parseRef first.

export interface AtUriParts {
  did: string;
  collection: string;
  rkey: string;
}

const AT_URI =
  /^at:\/\/(did:(?:plc:[a-z2-7]{24}|web:[a-z0-9.-]+))\/([a-zA-Z0-9.-]+)\/([A-Za-z0-9._:~-]+)$/;

export function parseAtUri(uri: string): AtUriParts | null {
  const m = AT_URI.exec(uri);
  if (!m?.[1] || !m[2] || !m[3]) return null;
  return { did: m[1], collection: m[2], rkey: m[3] };
}

export type RecordKind = "post" | "comment" | "sequence" | "forum" | "wikiRevision" | "other";

const OUR_NS = /^online\.7778777\.lessdugin(\.temp)?\.(post|comment|sequence|forum|wikiRevision)$/;

/** The kind of one of our collections, or `other` (including foreign ones). */
export function recordKind(collection: string): RecordKind {
  const m = OUR_NS.exec(collection);
  return (m?.[2] as RecordKind | undefined) ?? "other";
}

/** Whether hover previews apply (lfm-1.md §5.3): post, comment, sequence, wikiRevision, forum. */
export function previewEligible(collection: string): boolean {
  return recordKind(collection) !== "other";
}

export function isDid(s: string): boolean {
  return /^did:(plc:[a-z2-7]{24}|web:[a-z0-9]([a-z0-9.-]*[a-z0-9])?)$/.test(s);
}
