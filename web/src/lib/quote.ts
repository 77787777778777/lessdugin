// SPDX-License-Identifier: AGPL-3.0-or-later
// Quote anchoring for reacts and replies (lfm-1.md §11 "Writers"): the user's selection inside
// one rendered body becomes a `TextQuote` over that body's plain-text projection P.

import type { TextQuote } from "@lessdugin/engine/types";

const WS = /[\s\u{85}\u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}]+/gu;
const MAX_QUOTE_BYTES = 2000;
const CONTEXT_GRAPHEMES = 32;
const CONTEXT_BYTES = 640;

const segmenter =
  typeof Intl !== "undefined" && "Segmenter" in Intl
    ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
    : null;

function graphemes(s: string): string[] {
  return segmenter ? Array.from(segmenter.segment(s), (x) => x.segment) : Array.from(s);
}

function utf8(s: string): number {
  return new TextEncoder().encode(s).length;
}

function trimContext(parts: string[], fromStart: boolean): string {
  let p = fromStart ? parts.slice(0, CONTEXT_GRAPHEMES) : parts.slice(-CONTEXT_GRAPHEMES);
  while (p.length > 0 && utf8(p.join("")) > CONTEXT_BYTES)
    p = fromStart ? p.slice(0, -1) : p.slice(1);
  return p.join("");
}

/** Builds a quote of `selected` text against P (`text`). Returns null for an empty or
 * over-long selection. When the text is not found verbatim, only `exact` is set. */
export function quoteFor(text: string, selected: string): TextQuote | null {
  const exact = selected.normalize("NFC").replace(WS, " ").trim();
  if (exact === "" || utf8(exact) > MAX_QUOTE_BYTES) return null;
  const i = text.indexOf(exact);
  if (i < 0) return { exact };
  const prefix = trimContext(graphemes(text.slice(0, i)), false);
  const suffix = trimContext(graphemes(text.slice(i + exact.length)), true);
  const quote: TextQuote = { exact, start: Array.from(text.slice(0, i)).length };
  if (prefix !== "") quote.prefix = prefix;
  if (suffix !== "") quote.suffix = suffix;
  return quote;
}

/** The current selection as a quote, if it lies inside `root`. */
export function quoteFromSelection(root: Element | null, text: string): TextQuote | null {
  if (!root || typeof window === "undefined") return null;
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return null;
  const range = sel.getRangeAt(0);
  if (!root.contains(range.commonAncestorContainer)) return null;
  return quoteFor(text, sel.toString());
}
