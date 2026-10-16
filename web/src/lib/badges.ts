// SPDX-License-Identifier: AGPL-3.0-or-later
// Badge glyphs (lfm-1.md §9.1): badges render only in the fixed badge column, as ASCII
// bracketed words or as glyphs from this fixed set; the same glyphs are stripped from names.

export const BADGE_GLYPHS = [
  "\u{2611}",
  "\u{2612}",
  "\u{2705}",
  "\u{2713}",
  "\u{2714}",
  "\u{2715}",
  "\u{2716}",
  "\u{2717}",
  "\u{2718}",
  "\u{26a0}",
] as const;

export type BadgeGlyph = (typeof BADGE_GLYPHS)[number];

export const GLYPH = {
  check: "\u{2713}",
  cross: "\u{2717}",
  warning: "\u{26a0}",
} as const satisfies Record<string, BadgeGlyph>;
