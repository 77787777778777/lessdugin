// SPDX-License-Identifier: AGPL-3.0-or-later
// Display helpers for user-supplied names (threat-model.md SPA-5, TERM-2; lfm-1.md §9.1 Name).

import { BADGE_GLYPHS } from "./badges.ts";

const BIDI = /[\u{061c}\u{200e}\u{200f}\u{202a}-\u{202e}\u{2066}-\u{2069}]/gu;
const ZERO_WIDTH = /[\u{200b}-\u{200d}\u{2060}\u{feff}]/gu;
// biome-ignore lint/suspicious/noControlCharactersInRegex: removing control characters is the point
const CONTROLS = /[\u{0}-\u{8}\u{b}\u{c}\u{e}-\u{1f}\u{7f}-\u{9f}]/gu;
const WHITESPACE =
  /[\s\u{85}\u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}]+/gu;
const GLYPHS = new RegExp(`[${BADGE_GLYPHS.join("")}]`, "gu");

const segmenter =
  typeof Intl !== "undefined" && "Segmenter" in Intl
    ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
    : null;

/** Caps `s` at `max` grapheme clusters, ending with an ellipsis when cut. */
export function capGraphemes(s: string, max: number): string {
  const parts = segmenter ? Array.from(segmenter.segment(s), (x) => x.segment) : Array.from(s);
  if (parts.length <= max) return s;
  return `${parts.slice(0, max - 1).join("")}…`;
}

/** A name for display: bidi controls, zero-width characters, controls and badge glyphs
 * removed, whitespace collapsed, at most `max` graphemes. Rendered inside `<bdi>`. */
export function cleanName(s: string, max = 64): string {
  const cleaned = s
    .replace(BIDI, "")
    .replace(ZERO_WIDTH, "")
    .replace(CONTROLS, "")
    .replace(GLYPHS, "")
    .replace(WHITESPACE, " ")
    .trim();
  return capGraphemes(cleaned, max);
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
