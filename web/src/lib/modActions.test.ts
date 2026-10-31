// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Json, ModLogEntry } from "@lessdugin/engine/types";
import { describe, expect, it } from "vitest";
import { entryActionKey, recordActionValue, suspendsNotice, tagSlug } from "./modActions.ts";

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

  it("reads a #tag action, its head by uri (a stale head is asserted again, m3-11)", () => {
    const head = `at://did:plc:abcdefghijklmnopqrstuvwx/${NS}.wikiRevision/3kabc`;
    expect(recordActionValue(record(action("tag", { head: { uri: head, cid: "bafy" } })))).toEqual({
      kind: "tag",
      removed: false,
      head,
    });
    expect(recordActionValue(record(action("tag", { removed: true })))).toEqual({
      kind: "tag",
      removed: true,
    });
  });
});

describe("tagSlug", () => {
  it("names the slug of a tag: subject only", () => {
    expect(tagSlug("tag:decision-theory")).toBe("decision-theory");
    expect(tagSlug("tag:Decision")).toBeNull();
    expect(tagSlug("tag:")).toBeNull();
    expect(tagSlug(POST)).toBeNull();
  });
});

describe("suspendsNotice (social.md §4.5)", () => {
  it("passes on the engine's suspendsAssertions notice and nothing else", () => {
    expect(suspendsNotice([])).toBeNull();
    expect(suspendsNotice([{ code: "retroactive", message: "x" }])).toBeNull();
    expect(
      suspendsNotice([
        { code: "notEstablished", message: "recorded" },
        { code: "suspendsAssertions", message: "editing suspends a moderator's keep and its pin" },
      ]),
    ).toBe("editing suspends a moderator's keep and its pin");
  });
});
