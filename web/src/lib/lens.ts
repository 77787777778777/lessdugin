// SPDX-License-Identifier: AGPL-3.0-or-later
// Presentation rules of the lens (social.md §10.3) that the engine's views leave to clients.

import type { ItemState } from "@lessdugin/engine/types";

/** social.md §10.3 "Own newcomer items": the fold collapses a newcomer's items for every reader
 * (V8), but the newcomer sees their own `Collapsed(NewUser)` items expanded, with the label
 * kept, because the collapse is what OTHER readers see. */
export function ownNewcomerItem(
  state: ItemState,
  author: string,
  viewer: string | null | undefined,
): boolean {
  return (
    state.kind === "collapsed" &&
    state.reason === "newUser" &&
    viewer !== null &&
    viewer !== undefined &&
    author === viewer
  );
}
