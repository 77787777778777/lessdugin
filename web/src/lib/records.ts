// SPDX-License-Identifier: AGPL-3.0-or-later
// Reading the author's own record (from `raw(uri)`) back into a draft for an edit, so that an
// edit changes only what the user changed: the body's media list and a comment's quote are kept
// (data-model.md §5, `#media`, `#textQuote`). Record JSON is untrusted input, so every field is
// checked before use.

import type { AspectRatio, MediaRef, TextQuote } from "@lessdugin/engine/types";

type Obj = Record<string, unknown>;

const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const int = (v: unknown): number | undefined =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0 ? v : undefined;

function aspectRatio(v: unknown): AspectRatio | undefined {
  if (!isObj(v)) return undefined;
  const width = int(v.width);
  const height = int(v.height);
  return width && height ? { width, height } : undefined;
}

/** The body's `media` (`[defs#media]`) as draft media refs. An already-uploaded blob is
 * identified by its CID, which also serves as the ref id. */
export function mediaRefsOf(body: unknown): MediaRef[] {
  if (!isObj(body) || !Array.isArray(body.media)) return [];
  const out: MediaRef[] = [];
  for (const m of body.media) {
    if (!isObj(m) || !isObj(m.blob)) continue;
    const ref = isObj(m.blob.ref) ? str(m.blob.ref.$link) : undefined;
    const mime = str(m.blob.mimeType);
    const size = int(m.blob.size);
    if (!ref || !/^b[a-z2-7]{1,127}$/.test(ref) || !mime || size === undefined) continue;
    const r: MediaRef = { id: ref, cid: ref, mime, sizeBytes: size };
    const alt = str(m.alt);
    if (alt !== undefined) r.alt = alt;
    const ar = aspectRatio(m.aspectRatio);
    if (ar) r.aspectRatio = ar;
    const duration = int(m.durationMs);
    if (duration !== undefined) r.durationMs = duration;
    const name = str(m.name);
    if (name !== undefined) r.name = name;
    out.push(r);
  }
  return out;
}

/** A record's `#textQuote`, if well-formed. */
export function textQuoteOf(v: unknown): TextQuote | undefined {
  if (!isObj(v)) return undefined;
  const exact = str(v.exact);
  if (exact === undefined || exact === "") return undefined;
  const q: TextQuote = { exact };
  const prefix = str(v.prefix);
  if (prefix !== undefined) q.prefix = prefix;
  const suffix = str(v.suffix);
  if (suffix !== undefined) q.suffix = suffix;
  const start = int(v.start);
  if (start !== undefined) q.start = start;
  return q;
}

/** The markdown source of an inline body (`NS.markdown.text`), if present. */
export function markdownOf(body: unknown): string | undefined {
  return isObj(body) ? str(body.text) : undefined;
}

/** A string-valued field of a record. */
export function stringField(record: unknown, key: string): string | undefined {
  return isObj(record) ? str(record[key]) : undefined;
}

/** A string-array field of a record (non-strings dropped). */
export function stringsField(record: unknown, key: string): string[] {
  if (!isObj(record) || !Array.isArray(record[key])) return [];
  return (record[key] as unknown[]).filter((x): x is string => typeof x === "string");
}

/** `{ uri }` of a strong-ref field (`parent`, `root`). */
export function refUri(record: unknown, key: string): string | undefined {
  if (!isObj(record)) return undefined;
  const v = record[key];
  return isObj(v) ? str(v.uri) : undefined;
}

/** A field that is an object (for nested records such as `link` or `body`). */
export function objectField(record: unknown, key: string): Obj | undefined {
  if (!isObj(record)) return undefined;
  const v = record[key];
  return isObj(v) ? v : undefined;
}
