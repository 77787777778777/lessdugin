// SPDX-License-Identifier: AGPL-3.0-or-later
// Reading modAction records as the SPA needs them (data-model.md §6.7; social.md §4.2, §4.5):
// the action key a removal names, the action value of a stored record (to re-assert it on a
// new version), and the positive assertions in effect on an item, which an edit suspends.

import type { Json, ModActionValue, ModLogEntry, RawView } from "@lessdugin/engine/types";
import { parseAtUri } from "./uri.ts";

type Obj = { [k: string]: Json };

const isObj = (v: Json | undefined): v is Obj =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** The def name of a record's `action` (`…modAction#hide` → `hide`), or null. */
function actionName(action: Json | undefined): string | null {
  if (!isObj(action) || typeof action.$type !== "string") return null;
  const hash = action.$type.lastIndexOf("#");
  return hash < 0 ? null : action.$type.slice(hash + 1);
}

const isComment = (uri: string): boolean =>
  parseAtUri(uri)?.collection.endsWith(".comment") === true;

/** The action key a removal of the mod-log entry `e` names (data-model.md §6.7), or null for an
 * entry that is not a modAction (an edit or removal revision) or of an unknown action. */
export function entryActionKey(e: ModLogEntry): string | null {
  if (parseAtUri(e.uri)?.collection.endsWith(".modAction") !== true) return null;
  if (e.action.startsWith("pin:")) return e.action;
  switch (e.action) {
    case "hide":
    case "access":
    case "restrict":
    case "tier":
    case "lock":
    case "tag":
      return e.action;
    // a forum pin is on a post or sequence, a thread pin on a comment
    case "pin":
      return isComment(e.subject) ? "pin:thread" : "pin:forum";
    default:
      return null;
  }
}

/** The action value a stored modAction record asserts, with its fields' defaults (social.md
 * §4.2), or null for an action the SPA cannot write. */
export function recordActionValue(record: Json | undefined): ModActionValue | null {
  if (!isObj(record) || !isObj(record.action)) return null;
  const a = record.action;
  const bool = (k: string, d: boolean): boolean => (typeof a[k] === "boolean" ? a[k] : d);
  switch (actionName(a)) {
    case "hide":
      return {
        kind: "hide",
        hidden: bool("hidden", true),
        spam: bool("spam", false),
        showReason: bool("showReason", true),
      };
    case "tier":
      return a.tier === "personal" || a.tier === "frontpage" || a.tier === "curated"
        ? { kind: "tier", tier: a.tier }
        : null;
    case "pin":
      return a.scope === "forum" || a.scope === "thread"
        ? { kind: "pin", scope: a.scope, pinned: bool("pinned", true) }
        : null;
    case "lock":
      return { kind: "lock", locked: bool("locked", true) };
    default:
      return null;
  }
}

/** A positive assertion of social.md §4.2: a keep, a tier of frontpage or curated, a pin. */
export type PositiveKind = "keep" | "tier" | "pin";

/** The positive assertions in effect on the item of `raw` (status `effective`): an edit of the
 * item suspends them until an authority asserts them again (social.md §4.5). */
export function positiveAssertions(raw: RawView): PositiveKind[] {
  const out = new Set<PositiveKind>();
  for (const entry of raw.assertions) {
    if (!isObj(entry) || entry.status !== "effective") continue;
    const v = recordActionValue(entry.record);
    if (v?.kind === "hide" && !v.hidden) out.add("keep");
    else if (v?.kind === "tier" && v.tier !== "personal") out.add("tier");
    else if (v?.kind === "pin" && v.pinned) out.add("pin");
  }
  return [...out];
}

const POSITIVE_TEXT: Record<PositiveKind, string> = {
  keep: "a moderator's keep",
  tier: "its frontpage or curated tier",
  pin: "its pin",
};

/** What an author is told before editing an item that carries positive assertions (social.md
 * §4.5), or null when it carries none. */
export function suspensionWarning(
  kinds: readonly PositiveKind[],
  what: "post" | "comment",
): string | null {
  if (kinds.length === 0) return null;
  const list = kinds.map((k) => POSITIVE_TEXT[k]);
  const named =
    list.length === 1 ? list[0] : `${list.slice(0, -1).join(", ")} and ${list[list.length - 1]}`;
  return `Editing this ${what} suspends ${named}: they apply to the version a moderator saw, and stop until one asserts them again on the new version.`;
}
