// SPDX-License-Identifier: AGPL-3.0-or-later
// Quote anchoring for reacts and replies (lfm-1.md §11 "Writers"): the user's selection inside
// one rendered body becomes a `TextQuote` over that body's plain-text projection P.
//
// The selection is read from the DOM's text nodes the way P sees the body (lfm-1.md §10.1), not
// with `Selection.toString()`: that would include presentation the renderer or the SPA adds and
// P never contains (footnote reference numbers, `.link-host`, back-links, the spoiler summary,
// sidenote copies, media buttons, MathML glyphs), and the quote would be orphaned as soon as it
// was created. A selection that still does not occur verbatim in P (for example one spanning
// math, whose P is the TeX source) is reported as `unmatched`, so the UI can say so instead of
// sending a quote that resolves to "quoted text no longer present".

import type { TextQuote } from "@lessdugin/engine/types";

const WS = /[\s\u{85}\u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}]+/gu;
const MAX_QUOTE_BYTES = 2000;
const CONTEXT_GRAPHEMES = 32;
const CONTEXT_BYTES = 640;

export type QuoteSelection =
  /** Found in P: a full `TextQuote` with context and `start`. */
  | { kind: "anchored"; quote: TextQuote }
  /** Not a verbatim substring of P: the normalised selected text, for the warning. */
  | { kind: "unmatched"; exact: string };

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

/** Normalises like lfm-1.md §10.2 / §11 `norm(trim_ws(…))`: NFC, White_Space runs → U+0020. */
export function normaliseQuote(s: string): string {
  return s.normalize("NFC").replace(WS, " ").trim();
}

/** Builds a quote of `selected` text against P (`text`). Null for an empty or over-long
 * selection; `unmatched` when it does not occur verbatim in P. */
export function quoteFor(text: string, selected: string): QuoteSelection | null {
  const exact = normaliseQuote(selected);
  if (exact === "" || utf8(exact) > MAX_QUOTE_BYTES) return null;
  const i = text.indexOf(exact);
  if (i < 0) return { kind: "unmatched", exact };
  const prefix = trimContext(graphemes(text.slice(0, i)), false);
  const suffix = trimContext(graphemes(text.slice(i + exact.length)), true);
  const quote: TextQuote = { exact, start: Array.from(text.slice(0, i)).length };
  if (prefix !== "") quote.prefix = prefix;
  if (suffix !== "") quote.suffix = suffix;
  return { kind: "anchored", quote };
}

/** Elements whose text is not part of P (renderer presentation, §8.6) or was added by the SPA
 * after sanitising (sidenotes, media buttons). Images contribute their alt in P, but the DOM's
 * `alt` may come from the media record instead, so they are skipped too. */
const NOT_IN_P = [
  'sup[id^="md-fnref-"]',
  ".link-host",
  "a.fn-back",
  "details.spoiler > summary",
  "math",
  "input",
  "img",
  "button",
  ".lfm-sidenote",
].join(", ");

/** Elements that start a new line (or cell) in P; they contribute whitespace around them. */
const BREAKS = new Set([
  "p",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "li",
  "ol",
  "ul",
  "blockquote",
  "pre",
  "div",
  "section",
  "figure",
  "figcaption",
  "details",
  "summary",
  "table",
  "thead",
  "tbody",
  "tr",
  "th",
  "td",
  "hr",
  "br",
]);

const EXIT = Symbol("exit");

/** The text of `range` inside `root` as P sees it (before §10.2 normalisation). Iterative, so
 * deep bodies cannot exhaust the stack. */
export function projectedText(root: Element, range: Range): string {
  let out = "";
  const stack: (Node | typeof EXIT)[] = [root];
  while (stack.length > 0) {
    const item = stack.pop();
    if (item === undefined) break;
    if (item === EXIT) {
      out += " ";
      continue;
    }
    if (item.nodeType === 3) {
      if (!range.intersectsNode(item)) continue;
      const data = (item as Text).data;
      const start = item === range.startContainer ? range.startOffset : 0;
      const end = item === range.endContainer ? range.endOffset : data.length;
      out += data.slice(start, end);
      continue;
    }
    if (item.nodeType !== 1) continue;
    const el = item as Element;
    if (el !== root && (!range.intersectsNode(el) || el.matches(NOT_IN_P))) continue;
    if (BREAKS.has(el.localName)) {
      out += " ";
      stack.push(EXIT);
    }
    for (let c = el.lastChild; c; c = c.previousSibling) stack.push(c);
  }
  return out;
}

/** The current selection as a quote, if it lies inside `root`. */
export function quoteFromSelection(root: Element | null, text: string): QuoteSelection | null {
  if (!root || typeof window === "undefined") return null;
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return null;
  const range = sel.getRangeAt(0);
  if (!root.contains(range.commonAncestorContainer)) return null;
  return quoteFor(text, projectedText(root, range));
}
