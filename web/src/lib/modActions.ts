// SPDX-License-Identifier: AGPL-3.0-or-later
// Reading modAction records as the SPA needs them (data-model.md §6.7; social.md §4.2, §4.5):
// the action key a removal names, the action value of a stored record (to re-assert it on a
// new version), and the notice the engine gives before an edit that suspends assertions.

import type { ActionNotice, Json, ModActionValue, ModLogEntry } from "@lessdugin/engine/types";
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
    case "tag": {
      // `head` is a strongRef; asserting it again binds it to the revision's current CID
      const head = isObj(a.head) && typeof a.head.uri === "string" ? a.head.uri : undefined;
      return { kind: "tag", removed: bool("removed", false), ...(head ? { head } : {}) };
    }
    default:
      return null;
  }
}

/** The slug of a `"tag:" + slug` subject (data-model.md §6.7, G8), or null. */
export function tagSlug(subject: string): string | null {
  const slug = subject.startsWith("tag:") ? subject.slice(4) : "";
  return /^[a-z0-9]+(-[a-z0-9]+)*$/.test(slug) ? slug : null;
}

/** What the engine says an edit of the item suspends (`editPreflight`, engine-api.md §3.5): the
 * `suspendsAssertions` notice naming the positive assertions in effect on it, which the edit's
 * new version stops until an authority asserts them again (social.md §4.5); null when there is
 * none. The engine owns the rule, so every client warns about the same set. */
export function suspendsNotice(notices: readonly ActionNotice[]): string | null {
  const lines = notices.filter((n) => n.code === "suspendsAssertions").map((n) => n.message);
  return lines.length > 0 ? lines.join(" ") : null;
}
