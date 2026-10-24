// SPDX-License-Identifier: AGPL-3.0-or-later
// Action enablement from the fold's viewer capabilities (`ForumView.viewer`, social.md §8).
// The caps are UX only: writers still check every gate and the fold decides what counts, so an
// absent `viewer` (signed out, or the forum view is not loaded yet) never disables an action -
// the existing "sign in" affordance and the engine's own refusal carry that case.

import type { ViewerCaps } from "@lessdugin/engine/types";

/** A capability the client can gate an action on. */
export type Cap =
  | "vote"
  | "strongVote"
  | "agree"
  | "react"
  | "reactCreate"
  | "reactSecond"
  | "reactAnti"
  | "post"
  | "comment"
  | "moderate"
  | "curate"
  | "authorModPersonal"
  | "authorModFrontpage";

/** Whether `cap` allows the action. Unknown caps (no `viewer`) allow it; the fold decides. */
export function allows(caps: ViewerCaps | undefined, cap: Cap): boolean {
  return caps === undefined || caps[cap] === true;
}

/** Why a capped action is unavailable, for a button's `title` (social.md §8). */
export function capReason(caps: ViewerCaps | undefined, cap: Cap): string | undefined {
  if (allows(caps, cap)) return undefined;
  switch (cap) {
    case "vote":
    case "agree":
      return caps?.weighted === false
        ? "Your votes are recorded but carry no weight yet."
        : "You cannot vote in this forum.";
    case "strongVote":
      return "Strong votes are not available to you in this forum.";
    case "react":
      return "Reacts are not available to you in this forum.";
    case "reactCreate":
      return "You cannot add new reacts in this forum yet.";
    case "reactSecond":
      return "You cannot second reacts in this forum yet.";
    case "reactAnti":
      return "You cannot oppose reacts in this forum yet.";
    case "post":
      return "You cannot post in this forum.";
    case "comment":
      return "You cannot comment in this forum.";
    case "moderate":
      return "Only this forum's authorities can moderate.";
    case "curate":
      return "Only admins and the owner can curate.";
    case "authorModPersonal":
    case "authorModFrontpage":
      return "Author moderation is not available to you in this forum.";
  }
}

/** "Recorded; will count once you're established" (social.md §5–§7): the viewer is outside E. */
export function unweightedNote(caps: ViewerCaps | undefined): string | null {
  if (caps === undefined || caps.weighted) return null;
  return "Your votes are recorded and will count once you're established.";
}
