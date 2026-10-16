// SPDX-License-Identifier: AGPL-3.0-or-later
// The `default/1` react palette (docs/spec/reacts-default-1.json; social.md §9.1).
// Normative for the fold: the set of ids. Presentation defaults: labels, descriptions, glyphs,
// groups and their order.

import paletteJson from "../../../docs/spec/reacts-default-1.json";

export interface PaletteGroup {
  id: string;
  label: string;
}

export interface PaletteReact {
  id: string;
  label: string;
  description: string;
  glyph: string;
  group: string;
}

export interface Palette {
  id: string;
  groups: PaletteGroup[];
  reacts: PaletteReact[];
}

export const DEFAULT_PALETTE: Palette = {
  id: paletteJson.id,
  groups: paletteJson.groups,
  reacts: paletteJson.reacts,
};

const BY_ID = new Map(DEFAULT_PALETTE.reacts.map((r) => [r.id, r]));

export function paletteReact(id: string): PaletteReact | undefined {
  return BY_ID.get(id);
}
