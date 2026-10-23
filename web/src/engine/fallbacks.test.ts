// SPDX-License-Identifier: AGPL-3.0-or-later
// The draft stand-ins: composer text survives a reload (localStorage, hosted mode) or at least
// an unmount (memory, `lessdugin web`), robustness review H3.
import type { EngineApi } from "@lessdugin/engine/api";
import type { Draft } from "@lessdugin/engine/types";
import { describe, expect, it } from "vitest";
import { DRAFTS_MAX, memoryDrafts, storageDrafts, withFallbacks } from "./fallbacks.ts";

const ROOT = "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.post/3kabc";

const unsupported = async () => {
  throw { code: "unsupported", message: "not in M2", retryable: false };
};
const engine = {
  drafts: unsupported,
  saveDraft: unsupported,
  deleteDraft: unsupported,
} as unknown as EngineApi;

const comment = (markdown: string, id?: string): Draft => ({
  kind: "comment",
  ...(id !== undefined ? { id } : {}),
  comment: { root: ROOT, body: { markdown, media: [] } },
});

class MemStorage {
  data = new Map<string, string>();
  full = false;
  getItem(k: string) {
    return this.data.get(k) ?? null;
  }
  setItem(k: string, v: string) {
    if (this.full) throw new DOMException("full", "QuotaExceededError");
    this.data.set(k, v);
  }
}

describe("draft fallbacks", () => {
  it("save, update, list and delete when the engine cannot", async () => {
    const api = withFallbacks(engine, { drafts: memoryDrafts() });
    const id = await api.saveDraft(comment("first"));
    expect(await api.saveDraft(comment("second", id))).toBe(id);
    const list = await api.drafts();
    expect(list).toHaveLength(1);
    expect(list[0]?.draft).toEqual(comment("second", id));
    await api.deleteDraft(id);
    expect(await api.drafts()).toEqual([]);
  });

  it("keep drafts across reloads in localStorage, and ignore what is unreadable", async () => {
    const storage = new MemStorage();
    const a = withFallbacks(engine, { drafts: storageDrafts(storage as unknown as Storage) });
    const id = await a.saveDraft(comment("kept"));
    // a reload: a new app over the same storage
    const b = withFallbacks(engine, { drafts: storageDrafts(storage as unknown as Storage) });
    expect((await b.drafts()).map((d) => d.id)).toEqual([id]);
    storage.data.set("lessdugin-drafts-v1", '[{"id":1},{"nope":true}, "x"]');
    expect(await b.drafts()).toEqual([]);
    storage.data.set("lessdugin-drafts-v1", "{not json");
    expect(await b.drafts()).toEqual([]);
  });

  it("report a full storage and keep at most DRAFTS_MAX drafts", async () => {
    const storage = new MemStorage();
    const api = withFallbacks(engine, { drafts: storageDrafts(storage as unknown as Storage) });
    for (let i = 0; i < DRAFTS_MAX + 5; i++) await api.saveDraft(comment(`d${i}`));
    expect(await api.drafts()).toHaveLength(DRAFTS_MAX);
    storage.full = true;
    await expect(api.saveDraft(comment("more"))).rejects.toMatchObject({ code: "too_large" });
  });
});
