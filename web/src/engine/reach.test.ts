// SPDX-License-Identifier: AGPL-3.0-or-later
// docs/plan.md §M3 acceptance: "every MVP action is reachable in all three clients". REACH says
// where the SPA calls each operation of engine-api.md §3.2 (forums and membership), §3.5
// (writes) and §3.6 (local state), as the TUI's `REACH` table does (crates/tui/src/command.rs,
// checked by crates/tui/tests/social.rs): the spec's tables and this map must name the same
// operations, and each named source file must call its operation, so an operation the spec adds
// fails here until the SPA reaches it (or says why it does not).

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// vitest runs from web/
const SPEC = resolve(process.cwd(), "../docs/spec/engine-api.md");
const PLAN = resolve(process.cwd(), "../docs/plan.md");
const SRC = `${resolve(process.cwd(), "src")}/`;

/** How the SPA reaches an operation: a source file (under src/) that calls `engine.<op>(`
 * (`call` when the method is not the camelCased name), the result read from another operation's
 * call in that file (`through`), or not here, for the reason given (a later milestone, named
 * first: `M<n> (…)`). */
type Reach =
  | { file: string; call?: string }
  | { file: string; through: string; why: string }
  | { notHere: string };

/** The first milestone an exemption may name. M5 and M6 have landed their operations (the
 * engine implements them and the CLI and TUI reach them) while plan.md records their status, so
 * neither is "later" any more; M7 is the release. */
const FIRST_OPEN_MILESTONE = 7;

const REACH: Record<string, Reach> = {
  // §3.2
  forum_open: { file: "engine/queries.ts" },
  forum_discover: { file: "pages/HomePage.tsx" },
  forum_follow: { file: "pages/forum/ForumLayout.tsx" },
  forum_unfollow: { file: "pages/forum/ForumLayout.tsx" },
  followed: { file: "engine/queries.ts" },
  forum_create: { file: "pages/HomePage.tsx" },
  forum_update: { file: "pages/forum/ForumSettingsPage.tsx" },
  join: { file: "pages/forum/ForumLayout.tsx" },
  leave: { file: "pages/forum/ForumLayout.tsx" },
  members: { file: "pages/forum/ForumMembersPage.tsx" },
  grant: { file: "pages/forum/ForumMembersPage.tsx" },
  revoke: { file: "pages/forum/ForumMembersPage.tsx" },
  approve: { file: "pages/forum/ForumMembersPage.tsx" },
  reject: { file: "pages/forum/ForumMembersPage.tsx" },
  rejected_applicants: {
    file: "pages/forum/ForumMembersPage.tsx",
    through: "members",
    why: "`members { pending }` leaves out the applicants rejected on this device",
  },
  mods_set: { file: "pages/forum/ForumSettingsPage.tsx" },
  // §3.5
  publish: { file: "pages/forum/ForumNewPostPage.tsx" },
  comment: { file: "pages/PostPage.tsx" },
  edit: { file: "pages/forum/ForumNewPostPage.tsx" },
  edit_preflight: { file: "pages/forum/ForumNewPostPage.tsx" },
  delete: { file: "pages/PostPage.tsx" },
  retract: { file: "components/OwnCommentActions.tsx" },
  opinion: { file: "components/VoteControls.tsx" },
  mod_action: { file: "components/ModMenu.tsx" },
  mod_adopt: { file: "pages/forum/ForumModPage.tsx" },
  wiki_edit: { file: "pages/forum/WikiEditPage.tsx" },
  seq_create: { file: "pages/forum/ForumNewSequencePage.tsx" },
  seq_edit: { file: "pages/SequenceEditPage.tsx" },
  seq_add: { file: "pages/SequencePage.tsx" },
  seq_remove: { file: "pages/SequencePage.tsx" },
  seq_move: { file: "pages/SequencePage.tsx" },
  redact_revision: { file: "pages/PostPage.tsx" },
  prepare_media: { file: "components/Attachments.tsx" },
  undo: { file: "pages/OutboxPage.tsx" },
  retry_write: { file: "pages/OutboxPage.tsx" },
  write_status: {
    file: "pages/OutboxPage.tsx",
    through: "writes",
    why: "write events and `writes()` drive the outbox page and the pending badges",
  },
  writes: { file: "pages/OutboxPage.tsx" },
  // §3.6
  mark_read: { file: "pages/PostPage.tsx" },
  mark_notifications_read: { file: "pages/NotificationsPage.tsx" },
  mark_all_notifications_read: { file: "pages/NotificationsPage.tsx" },
  bookmark: { file: "pages/PostPage.tsx" },
  subscribe: { file: "pages/PostPage.tsx", call: "setSubscription" },
  mute: { file: "pages/UserPage.tsx" },
  acknowledged_public_actions: {
    file: "components/PublicAck.tsx",
    through: "settings",
    why: "`Settings.acknowledgedPublicActions` carries them",
  },
  public_action_acknowledged: {
    file: "components/PublicAck.tsx",
    through: "settings",
    why: "the interstitial reads `Settings.acknowledgedPublicActions`",
  },
  acknowledge_public_action: { file: "components/PublicAck.tsx" },
  karma_notifications: {
    file: "engine/queries.ts",
    through: "settings",
    why: "`Settings.karmaNotifications` carries it (`useSettings`, the settings page)",
  },
  set_karma_notifications: { file: "pages/SettingsPage.tsx" },
  save_draft: { file: "components/Composer.tsx" },
  delete_draft: { file: "components/Composer.tsx" },
  settings: { file: "engine/queries.ts" },
  set_setting: { file: "pages/SettingsPage.tsx" },
  export_local: { file: "pages/SettingsPage.tsx" },
  import_local: { file: "pages/SettingsPage.tsx" },
};

/** The operations of §3.2, §3.5 and §3.6, by name, from the spec's tables (as the TUI's
 * `spec_operations` reads them): each backticked code of a row's first cell names one. */
function specOperations(): Set<string> {
  const ops = new Set<string>();
  let on = false;
  for (const line of readFileSync(SPEC, "utf8").split("\n")) {
    if (line.startsWith("### ")) {
      on = ["### 3.2 ", "### 3.5 ", "### 3.6 "].some((h) => line.startsWith(h));
      continue;
    }
    if (!on || !line.startsWith("| `")) continue;
    const first = line.split("|")[1] ?? "";
    for (const code of first.split("`").filter((_, i) => i % 2 === 1)) {
      const name = /^[A-Za-z0-9_]+/.exec(code)?.[0] ?? "";
      if (name !== "") ops.add(name);
    }
  }
  return ops;
}

/** The milestones plan.md marks done: a `**Status:** done` line under `### M<n>`. */
function doneMilestones(): Set<number> {
  const done = new Set<number>();
  let current: number | null = null;
  for (const line of readFileSync(PLAN, "utf8").split("\n")) {
    const heading = /^### M(\d+)\b/.exec(line);
    if (heading) current = Number(heading[1]);
    else if (line.startsWith("#")) current = null;
    else if (current !== null && line.startsWith("**Status:** done")) done.add(current);
  }
  return done;
}

const camel = (op: string): string => op.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());

describe("every engine operation has a way in (docs/plan.md §M3)", () => {
  const spec = specOperations();

  it("names the operations of the spec's tables, and nothing else", () => {
    expect(spec.size).toBeGreaterThan(40);
    const reach = new Set(Object.keys(REACH));
    expect([...spec].filter((op) => !reach.has(op))).toEqual([]);
    expect([...reach].filter((op) => !spec.has(op))).toEqual([]);
  });

  it("calls each reachable operation where it says", () => {
    const missing: string[] = [];
    for (const [op, r] of Object.entries(REACH)) {
      if ("notHere" in r) continue;
      const method = "through" in r ? r.through : (r.call ?? camel(op));
      const source = readFileSync(`${SRC}${r.file}`, "utf8");
      // `engine.method(`, the method on its own line after a long receiver included
      if (!new RegExp(`\\bengine\\s*\\.\\s*${method}\\(`).test(source)) {
        missing.push(`${op} in ${r.file}`);
      }
    }
    expect(missing).toEqual([]);
  });

  it("leaves out only the operations of a milestone that has not landed", () => {
    const done = doneMilestones();
    // the plan's status lines are read: M0 to M4 are done
    expect([0, 1, 2, 3, 4].every((m) => done.has(m))).toBe(true);
    for (const [op, r] of Object.entries(REACH)) {
      if (!("notHere" in r)) continue;
      const m = /^M(\d+)\b/.exec(r.notHere);
      expect(m, `${op}: ${r.notHere}`).not.toBeNull();
      const n = Number(m?.[1]);
      expect(done.has(n), `${op} names done milestone M${n}`).toBe(false);
      expect(n, `${op} names landed milestone M${n}`).toBeGreaterThanOrEqual(FIRST_OPEN_MILESTONE);
    }
  });
});
