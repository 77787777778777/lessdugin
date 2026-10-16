// SPDX-License-Identifier: AGPL-3.0-or-later
// Helpers over canonical at-uris (data-model.md §2.3). They only split strings that came from
// the engine; anything that came from content is re-parsed by the engine's parseRef first.
// The syntax checks mirror lfm-1.md §5.2 `parse_at_link` (did:plc / did:web, NSID, record key)
// and are shared with the sanitiser's value grammars (lib/sanitize.ts).

export interface AtUriParts {
  did: string;
  collection: string;
  rkey: string;
}

/** A lowercase DNS label, and a hostname of one or more of them (no port, no path). */
const LABEL = "[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?";
const HOSTNAME = `${LABEL}(?:\\.${LABEL})*`;
/** `did:plc:` + 24 base32 characters, or `did:web:` + a lowercase hostname (lfm-1.md §5.2). */
const DID = `did:(?:plc:[a-z2-7]{24}|web:${HOSTNAME})`;
/** atproto NSID: reversed domain authority (≥ 2 segments) plus a name segment. */
const NSID_SEGMENT = "[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?";
const NSID = `[a-zA-Z](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\\.${NSID_SEGMENT})+\\.[a-zA-Z][a-zA-Z0-9]{0,62}`;
/** atproto record key: 1–512 of `[A-Za-z0-9._:~-]`, but never `.` or `..`. */
const RKEY = "(?!\\.{1,2}(?:$|/))[A-Za-z0-9._:~-]{1,512}";

const DID_RE = new RegExp(`^${DID}$`);
const AT_URI_RE = new RegExp(`^at://(${DID})/(${NSID})/(${RKEY})$`);

export function parseAtUri(uri: string): AtUriParts | null {
  if (uri.length > 8192) return null;
  const m = AT_URI_RE.exec(uri);
  if (!m?.[1] || !m[2] || !m[3]) return null;
  return { did: m[1], collection: m[2], rkey: m[3] };
}

/** A canonical record at-uri: `at://<did>/<nsid>/<rkey>` (lfm-1.md §5.2 `Record`). */
export function isRecordUri(uri: string): boolean {
  return parseAtUri(uri) !== null;
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
  return s.length <= 2048 && DID_RE.test(s);
}
