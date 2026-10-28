// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Json, ModLogEntry, RawView } from "@lessdugin/engine/types";
import { DTO_VERSION } from "@lessdugin/engine/types";
import { describe, expect, it } from "vitest";
import {
  entryActionKey,
  positiveAssertions,
  recordActionValue,
  suspensionWarning,
} from "./modActions.ts";

const NS = "online.7778777.lessdugin.temp";
const COMMENT = `at://did:plc:abcdefghijklmnopqrstuvwx/${NS}.comment/3kabc`;
const POST = `at://did:plc:abcdefghijklmnopqrstuvwx/${NS}.post/3kabc`;
const action = (name: string, fields: { [k: string]: Json } = {}): Json => ({
  $type: `${NS}.modAction#${name}`,
  ...fields,
});
const record = (a: Json): Json => ({ $type: `${NS}.modAction`, subject: POST, action: a });

function entry(over: Partial<ModLogEntry>): ModLogEntry {
  return {
    uri: `at://did:plc:abcdefghijklmnopqrstuvwx/${NS}.modAction/k-1`,
    actor: { did: "did:plc:abcdefghijklmnopqrstuvwx", shortDid: "x", handleState: "verified" },
    action: "hide",
    subject: POST,
    value: "",
    atMs: 0,
    status: "effective",
    lapsed: false,
    ...over,
  };
}

describe("entryActionKey (data-model.md §6.7)", () => {
  it("names the action key of a modAction entry, a pin by its subject's kind", () => {
    expect(entryActionKey(entry({ action: "access" }))).toBe("access");
    expect(entryActionKey(entry({ action: "pin", subject: POST }))).toBe("pin:forum");
    expect(entryActionKey(entry({ action: "pin", subject: COMMENT }))).toBe("pin:thread");
    expect(entryActionKey(entry({ action: "pin:thread" }))).toBe("pin:thread");
  });

  it("names none for a revision entry or an unknown action", () => {
    expect(
      entryActionKey(
        entry({ uri: `at://did:plc:abcdefghijklmnopqrstuvwx/${NS}.revision/r`, action: "edit" }),
      ),
    ).toBeNull();
    expect(entryActionKey(entry({ action: "unknown" }))).toBeNull();
  });
});

describe("recordActionValue", () => {
  it("reads a stored action with its fields' defaults (social.md §4.2)", () => {
    expect(recordActionValue(record(action("hide")))).toEqual({
      kind: "hide",
      hidden: true,
      spam: false,
      showReason: true,
    });
    expect(recordActionValue(record(action("pin", { scope: "thread" })))).toEqual({
      kind: "pin",
      scope: "thread",
      pinned: true,
    });
    expect(recordActionValue(record(action("tier", { tier: "curated" })))).toEqual({
      kind: "tier",
      tier: "curated",
    });
    expect(recordActionValue(record(action("tier", { tier: "gold" })))).toBeNull();
  });
});

describe("positiveAssertions (social.md §4.5)", () => {
  const raw = (assertions: Json[]): RawView => ({
    dto: DTO_VERSION,
    uri: POST,
    level: "pds",
    validity: [],
    assertions,
  });
  const a = (act: Json, status = "effective"): Json => ({
    uri: "at://x",
    cid: "c",
    valid: true,
    record: record(act),
    status,
  });

  it("lists the keeps, tiers above personal and pins in effect", () => {
    const view = raw([
      a(action("hide", { hidden: false })),
      a(action("tier", { tier: "frontpage" })),
      a(action("pin", { scope: "forum" })),
    ]);
    expect(positiveAssertions(view)).toEqual(["keep", "tier", "pin"]);
  });

  it("ignores negative assertions and ones not in effect", () => {
    const view = raw([
      a(action("hide")),
      a(action("tier", { tier: "personal" })),
      a(action("pin", { scope: "forum", pinned: false })),
      a(action("hide", { hidden: false }), "stale"),
      a(action("tier", { tier: "curated" }), "overridden"),
    ]);
    expect(positiveAssertions(view)).toEqual([]);
  });

  it("words the warning an author sees before an edit", () => {
    expect(suspensionWarning([], "post")).toBeNull();
    expect(suspensionWarning(["keep", "pin"], "comment")).toMatch(
      /^Editing this comment suspends a moderator's keep and its pin:/,
    );
  });
});
