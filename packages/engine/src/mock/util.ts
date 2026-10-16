// SPDX-License-Identifier: AGPL-3.0-or-later
// Deterministic helpers for the mock engine: no Math.random, no wall clock.

import type { AtUri, Did, RenderedBody, TocEntry } from "../types.ts";

/** The development namespace (data-model.md §1.3); the mock behaves like a `.temp` build. */
export const NS = "online.7778777.lessdugin.temp";
export const COLL = {
  forum: `${NS}.forum`,
  membership: `${NS}.membership`,
  post: `${NS}.post`,
  comment: `${NS}.comment`,
  revision: `${NS}.revision`,
  opinion: `${NS}.opinion`,
  modAction: `${NS}.modAction`,
  wikiRevision: `${NS}.wikiRevision`,
  sequence: `${NS}.sequence`,
  profile: `${NS}.profile`,
  markdown: `${NS}.markdown`,
} as const;

/** Renderer version reported by the mock's bodies. */
export const MOCK_RENDERER_VERSION = "mock/1";

const S32 = "234567abcdefghijklmnopqrstuvwxyz";

/** FNV-1a over UTF-16 code units, 32-bit, with a seed. Not cryptographic; deterministic. */
function fnv1a(input: string, seed: number): number {
  let h = (0x811c9dc5 ^ seed) >>> 0;
  for (let i = 0; i < input.length; i++) {
    h ^= input.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  // final avalanche (murmur3 fmix32)
  h ^= h >>> 16;
  h = Math.imul(h, 0x85ebca6b) >>> 0;
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35) >>> 0;
  h ^= h >>> 16;
  return h >>> 0;
}

/** `len` base32-sortable characters derived from `input`. */
export function b32hash(input: string, len: number): string {
  let out = "";
  let seed = 0;
  while (out.length < len) {
    let h = fnv1a(input, seed++);
    for (let i = 0; i < 6 && out.length < len; i++) {
      out += S32[h & 31];
      h >>>= 5;
    }
  }
  return out;
}

/** An atproto TID for a millisecond timestamp (µs << 10 | clock id), 13 characters. */
export function tid(ms: number, clockId = 0): string {
  let v = ((BigInt(Math.floor(ms)) * 1000n) << 10n) | BigInt(clockId & 1023);
  let out = "";
  for (let i = 0; i < 13; i++) {
    out = S32[Number(v & 31n)] + out;
    v >>= 5n;
  }
  return out;
}

/** A plausible CIDv1 (dag-cbor, sha2-256) string derived from `input`. */
export function fakeCid(input: string): string {
  return `bafyrei${b32hash(`cid:${input}`, 52)}`;
}

/** A plausible raw-codec CID for blobs. */
export function fakeBlobCid(input: string): string {
  return `bafkrei${b32hash(`blob:${input}`, 52)}`;
}

export function didFor(name: string): Did {
  return `did:plc:${b32hash(`actor:${name}`, 24)}`;
}

/** lfm-1.md §5.4 `short_did`. */
export function shortDid(did: Did): string {
  if (did.startsWith("did:plc:")) return `did:plc:${did.slice(8, 16)}…`;
  if (did.length <= 40) return did;
  return `${did.slice(0, 39)}…`;
}

export function atUri(did: Did, collection: string, rkey: string): AtUri {
  return `at://${did}/${collection}/${rkey}`;
}

export interface ParsedAtUri {
  did: string;
  collection: string;
  rkey: string;
}

export function splitAtUri(uri: string): ParsedAtUri | null {
  const m = /^at:\/\/([^/]+)\/([^/]+)\/([^/]+)$/.exec(uri);
  if (!m?.[1] || !m[2] || !m[3]) return null;
  return { did: m[1], collection: m[2], rkey: m[3] };
}

export function escapeText(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\u{a0}/gu, "&nbsp;");
}

export function escapeAttr(s: string): string {
  return escapeText(s).replace(/"/g, "&quot;");
}

function decodeEntities(s: string): string {
  return s
    .replace(/&nbsp;/g, "\u{a0}")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, "&");
}

const WS_RUN =
  /[\t\n\v\f\r \u{85}\u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}]+/gu;

/** lfm-1.md §10.2 normalisation (NFC, collapse White_Space, trim one space each side). */
export function normalizeP(s: string): string {
  return s.normalize("NFC").replace(WS_RUN, " ").replace(/^ /, "").replace(/ $/, "");
}

const BLOCK_TAG =
  /<\/?(?:p|h[1-6]|li|ul|ol|div|pre|blockquote|details|summary|section|table|thead|tbody|tr|th|td|figure|figcaption|hr|br)\b[^>]*>/g;

/** An approximation of P computed from LFM HTML (the mock has no markdown parser for every
 * construct). Footnote back-links are dropped; math contributes its text content. */
export function htmlToText(html: string, opts: { omitSpoilers?: boolean } = {}): string {
  let s = html.replace(/<a data-anchor="md-fnref-[^"]*" class="fn-back">[^<]*<\/a>/g, "");
  s = s.replace(/<sup id="md-fnref-[^"]*">.*?<\/sup>/g, "");
  if (opts.omitSpoilers) {
    s = s.replace(/<details class="spoiler">[\s\S]*?<\/details>/g, " ");
  }
  s = s.replace(/<span class="link-host">[^<]*<\/span>/g, "");
  s = s.replace(BLOCK_TAG, " ").replace(/<[^>]+>/g, "");
  return normalizeP(decodeEntities(s));
}

/** lfm-1.md §10.4, with code points standing in for grapheme clusters. */
export function excerptOf(px: string): string {
  const cps = Array.from(px);
  if (cps.length <= 500) return px;
  let head = cps.slice(0, 499).join("");
  const k = head.lastIndexOf(" ");
  if (k > 0) head = head.slice(0, k);
  return `${head}…`;
}

export function wordCount(p: string): number {
  if (p === "") return 0;
  return p.split(" ").length;
}

export function readMinutes(words: number): number {
  return words === 0 ? 0 : Math.max(1, Math.floor((words + 125) / 250));
}

/** Top-level headings of LFM HTML, for the ToC (lfm-1.md §10.6). */
export function tocOf(html: string): TocEntry[] {
  const out: TocEntry[] = [];
  // Only headings directly inside the root (the mock never nests them in containers).
  const re = /<h([1-6]) id="([^"]*)" dir="auto">([\s\S]*?)<\/h\1>/g;
  for (const m of html.matchAll(re)) {
    const level = Number(m[1]);
    const id = m[2] ?? "";
    out.push({ level, id, text: htmlToText(m[3] ?? "") });
  }
  return out;
}

/** Builds a `RenderedBody` from LFM HTML. */
export function bodyFromHtml(html: string, notices: string[] = []): RenderedBody {
  const text = htmlToText(html);
  const words = wordCount(text);
  return {
    html,
    text,
    toc: tocOf(html),
    wordCount: words,
    readMinutes: readMinutes(words),
    media: [],
    excerpt: excerptOf(htmlToText(html, { omitSpoilers: true })),
    notices,
    rendererVersion: MOCK_RENDERER_VERSION,
  };
}

export function utf8Length(s: string): number {
  return new TextEncoder().encode(s).length;
}

export function clone<T>(v: T): T {
  return structuredClone(v);
}
