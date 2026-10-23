// SPDX-License-Identifier: AGPL-3.0-or-later
// The browser KV stores: IndexedDB (over fake-indexeddb) against the in-memory reference, the
// unsigned byte-wise key order the core's tuple keys rely on, and the atomicity of a batch.
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { describe, expect, it } from "vitest";
import {
  Connection,
  compareBytes,
  type IDBFactoryLike,
  IdbKv,
  type KvBackend,
  MemKv,
  OP_DELETE,
  OP_DELETE_RANGE,
  OP_PUT,
  storageError,
} from "./idb.ts";
import { isHostError } from "./protocol.ts";

const b = (...xs: number[]): Uint8Array => Uint8Array.from(xs);
const hex = (k: Uint8Array): string =>
  Array.from(k, (x) => x.toString(16).padStart(2, "0")).join("");

let dbn = 0;
function fakeIdb(): IDBFactoryLike {
  return { factory: new IDBFactory(), keyRange: IDBKeyRange };
}
async function openIdb(durability: "strict" | "relaxed" = "strict"): Promise<IdbKv> {
  dbn += 1;
  return IdbKv.open(`test-${dbn}`, durability, fakeIdb());
}

async function scanKeys(
  s: KvBackend,
  lower?: Uint8Array,
  upper?: Uint8Array,
  opts: { lowerOpen?: boolean; upperOpen?: boolean; reverse?: boolean; limit?: number } = {},
): Promise<string[]> {
  const flat = await s.scan(
    lower,
    opts.lowerOpen ?? false,
    upper,
    opts.upperOpen ?? false,
    opts.reverse ?? false,
    opts.limit ?? 1000,
  );
  const out: string[] = [];
  for (let i = 0; i < flat.length; i += 2) out.push(hex(flat[i] as Uint8Array));
  return out;
}

// Keys whose signed and unsigned orders differ, and prefixes (the core writes no empty key).
const EDGE_KEYS = [
  b(0x00),
  b(0x00, 0x00),
  b(0x00, 0xff),
  b(0x01),
  b(0x01, 0x00),
  b(0x7f),
  b(0x7f, 0xff, 0xff),
  b(0x80),
  b(0x80, 0x00),
  b(0xfe, 0xff),
  b(0xff),
  b(0xff, 0x00),
  b(0xff, 0xff),
];

function sortedHex(keys: Uint8Array[]): string[] {
  return [...keys].sort(compareBytes).map(hex);
}

/** mulberry32 */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

describe("compareBytes", () => {
  it("orders unsigned and prefix first", () => {
    expect(compareBytes(b(0x7f), b(0x80))).toBeLessThan(0);
    expect(compareBytes(b(0x01), b(0x01, 0x00))).toBeLessThan(0);
    expect(compareBytes(b(), b(0x00))).toBeLessThan(0);
    expect(compareBytes(b(0xff, 0x00), b(0xff, 0x00))).toBe(0);
  });
});

for (const [name, open] of [
  ["MemKv", async (): Promise<KvBackend> => new MemKv()],
  ["IdbKv", (): Promise<KvBackend> => openIdb()],
] as const) {
  describe(name, () => {
    it("scans in unsigned byte-wise order, both directions", async () => {
      const s = await open();
      const shuffled = [...EDGE_KEYS].reverse();
      await s.write(shuffled.flatMap((k, i) => [OP_PUT, k, b(i)]));
      const want = sortedHex(EDGE_KEYS);
      expect(await scanKeys(s)).toEqual(want);
      expect(await scanKeys(s, undefined, undefined, { reverse: true })).toEqual(
        [...want].reverse(),
      );
      expect(await scanKeys(s, undefined, undefined, { limit: 3 })).toEqual(want.slice(0, 3));
      expect(await scanKeys(s, undefined, undefined, { limit: 3, reverse: true })).toEqual(
        [...want].reverse().slice(0, 3),
      );
    });

    it("honours open and closed bounds", async () => {
      const s = await open();
      await s.write(EDGE_KEYS.flatMap((k) => [OP_PUT, k, b(1)]));
      expect(await scanKeys(s, b(0x01), b(0x80))).toEqual(["01", "0100", "7f", "7fffff", "80"]);
      expect(await scanKeys(s, b(0x01), b(0x80), { lowerOpen: true, upperOpen: true })).toEqual([
        "0100",
        "7f",
        "7fffff",
      ]);
      // a tuple-style prefix range: everything under 0xff, excluding 0xff itself
      expect(await scanKeys(s, b(0xff), undefined, { lowerOpen: true })).toEqual(["ff00", "ffff"]);
      expect(await scanKeys(s, undefined, b(0x00), { reverse: true })).toEqual(["00"]);
      // zero-length bounds: below every key
      expect(await scanKeys(s, b(), b(0x00, 0x00))).toEqual(["00", "0000"]);
      expect(await scanKeys(s, b(), b(0x00), { lowerOpen: true })).toEqual(["00"]);
      expect(await scanKeys(s, undefined, b())).toEqual([]);
      expect(await scanKeys(s, undefined, b(), { upperOpen: true })).toEqual([]);
    });

    it("selects nothing for a limit of 0, an inverted or an empty range", async () => {
      const s = await open();
      await s.write(EDGE_KEYS.flatMap((k) => [OP_PUT, k, b(1)]));
      expect(await scanKeys(s, undefined, undefined, { limit: 0 })).toEqual([]);
      expect(await scanKeys(s, b(0x80), b(0x01))).toEqual([]);
      expect(await scanKeys(s, b(0x80), b(0x01), { reverse: true })).toEqual([]);
      expect(await scanKeys(s, b(0x80), b(0x80), { lowerOpen: true })).toEqual([]);
      expect(await scanKeys(s, b(0x80), b(0x80))).toEqual(["80"]);
      // an inverted delete range deletes nothing
      await s.write([OP_DELETE_RANGE, b(0x80), false, b(0x01), false]);
      expect(await scanKeys(s)).toHaveLength(EDGE_KEYS.length);
    });

    it("gets many in request order and copies values", async () => {
      const s = await open();
      await s.write([OP_PUT, b(0x02), b(2), OP_PUT, b(0x01), b(1)]);
      const got = await s.getMany([b(0x01), b(0x03), b(0x02), b(0x01)]);
      expect(got.map((v) => (v ? hex(v) : null))).toEqual(["01", null, "02", "01"]);
      expect(await s.getMany([])).toEqual([]);
      const v = got[0] as Uint8Array;
      v[0] = 9;
      expect(hex((await s.getMany([b(0x01)]))[0] as Uint8Array)).toBe("01");
    });

    it("applies a batch in order: put, overwrite, delete, delete range, unbounded range", async () => {
      const s = await open();
      await s.write([
        OP_PUT,
        b(0x01),
        b(1),
        OP_PUT,
        b(0x01),
        b(2),
        OP_PUT,
        b(0x02),
        b(3),
        OP_DELETE,
        b(0x02),
        OP_PUT,
        b(0x02),
        b(4),
        OP_PUT,
        b(0x10),
        b(5),
        OP_PUT,
        b(0x10, 0x01),
        b(6),
        OP_PUT,
        b(0x11),
        b(7),
        OP_DELETE_RANGE,
        b(0x10),
        false,
        b(0x11),
        true,
        OP_DELETE,
        b(0x99),
      ]);
      const flat = await s.scan(undefined, false, undefined, false, false, 100);
      expect(flat.map(hex)).toEqual(["01", "02", "02", "04", "11", "07"]);
      await s.write([OP_DELETE_RANGE, null, false, null, false]);
      expect(await scanKeys(s)).toEqual([]);
    });

    it("rejects a malformed batch as a whole", async () => {
      const s = await open();
      await s.write([OP_PUT, b(0x01), b(1)]);
      for (const bad of [
        [OP_PUT, b(0x02), b(2), 7, b(0x03)],
        [OP_DELETE, b(0x01), OP_PUT, "not bytes", b(2)],
        [OP_PUT, b(0x02), b(2), OP_DELETE_RANGE, b(0x00), false, 42, false],
      ]) {
        const err = await s.write(bad).then(
          () => null,
          (e: unknown) => e,
        );
        expect(isHostError(err) && err.kind).toBe("storage");
        expect(await scanKeys(s)).toEqual(["01"]);
      }
    });

    it("agrees with the in-memory reference under random batches", async () => {
      const s = await open();
      const ref = new MemKv();
      const r = rng(name.length * 7919);
      const bytes = (min: number): Uint8Array => {
        const alphabet = [0x00, 0x01, 0x7f, 0x80, 0xff];
        const n = min + Math.floor(r() * (4 - min));
        return Uint8Array.from({ length: n }, () => alphabet[Math.floor(r() * 5)] as number);
      };
      const key = (): Uint8Array => bytes(1);
      const maybe = (): Uint8Array | undefined => (r() < 0.2 ? undefined : bytes(0));
      for (let round = 0; round < 40; round++) {
        const ops: unknown[] = [];
        for (let i = 0; i < 6; i++) {
          const x = r();
          if (x < 0.6) ops.push(OP_PUT, key(), b(round, i));
          else if (x < 0.85) ops.push(OP_DELETE, key());
          else ops.push(OP_DELETE_RANGE, maybe(), r() < 0.5, maybe(), r() < 0.5);
        }
        await s.write(ops);
        await ref.write(ops);
        for (let q = 0; q < 4; q++) {
          const args = [maybe(), r() < 0.5, maybe(), r() < 0.5, r() < 0.5, 1 + Math.floor(r() * 6)];
          const [lo, lop, hi, hip, rev, lim] = args as [
            Uint8Array | undefined,
            boolean,
            Uint8Array | undefined,
            boolean,
            boolean,
            number,
          ];
          const got = (await s.scan(lo, lop, hi, hip, rev, lim)).map(hex);
          const want = (await ref.scan(lo, lop, hi, hip, rev, lim)).map(hex);
          expect(got).toEqual(want);
        }
      }
      expect(await scanKeys(s)).toEqual(await scanKeys(ref));
    });
  });
}

describe("IdbKv", () => {
  it("keeps data across reopen and separates databases", async () => {
    const idb = fakeIdb();
    const a = await IdbKv.open("state", "strict", idb);
    await a.write([OP_PUT, b(0x01), b(1)]);
    a.close();
    const again = await IdbKv.open("state", "strict", idb);
    expect(await scanKeys(again)).toEqual(["01"]);
    const other = await IdbKv.open("cache", "relaxed", idb);
    expect(await scanKeys(other)).toEqual([]);
  });

  it("refuses the empty key without applying the batch", async () => {
    const s = await openIdb();
    const err = await s.write([OP_PUT, b(0x01), b(1), OP_PUT, b(), b(2)]).then(
      () => null,
      (e: unknown) => e,
    );
    expect(isHostError(err) && err.kind).toBe("storage");
    expect(await scanKeys(s)).toEqual([]);
    expect(await s.getMany([b()])).toEqual([undefined]);
    await s.write([OP_PUT, b(0x01), b(1), OP_DELETE, b(), OP_DELETE_RANGE, b(), false, b(), false]);
    expect(await scanKeys(s)).toEqual(["01"]);
  });

  it("reports a missing IndexedDB as a storage error", async () => {
    const err = await IdbKv.open("x", "strict", null).then(
      () => null,
      (e: unknown) => e,
    );
    expect(isHostError(err) && err.kind).toBe("storage");
  });
});

describe("IndexedDB recovery (robustness review L6)", () => {
  function open(factory: IDBFactory, name: string, version: number): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
      const r = factory.open(name, version);
      r.onupgradeneeded = () => {
        if (!r.result.objectStoreNames.contains("kv")) r.result.createObjectStore("kv");
      };
      r.onsuccess = () => resolve(r.result);
      r.onerror = () => reject(r.error);
      r.onblocked = () => reject(new Error("blocked"));
    });
  }

  it("lets another tab delete or upgrade the database, then opens it again", async () => {
    const idb = fakeIdb();
    const s = await IdbKv.open("state", "strict", idb);
    await s.write([OP_PUT, b(0x01), b(1)]);
    // another tab deletes it (the versionchange closes this connection, so it is not blocked)
    await new Promise<void>((resolve, reject) => {
      const r = idb.factory.deleteDatabase("state");
      r.onsuccess = () => resolve();
      r.onerror = () => reject(r.error);
      r.onblocked = () => reject(new Error("the delete was blocked"));
    });
    // the next operation reopens a fresh database
    expect(await scanKeys(s)).toEqual([]);
    await s.write([OP_PUT, b(0x02), b(2)]);
    expect(await scanKeys(s)).toEqual(["02"]);
    // a newer build upgrades the schema: this one asks for a reload
    const newer = await open(idb.factory, "state", 2);
    const err = await s.getMany([b(0x02)]).then(
      () => null,
      (e: unknown) => e,
    );
    expect(isHostError(err) && err.message).toContain("reload this page");
    newer.close();
  });

  it("opens a connection the browser closed again", async () => {
    const idb = fakeIdb();
    const db = await open(idb.factory, "c", 1);
    const conn = new Connection(db, () => open(idb.factory, "c", 1));
    db.close(); // as the browser does on "clear site data" (its close event may come later)
    const tx = await conn.transaction("kv", "readwrite");
    tx.objectStore("kv").put(b(1), b(0x01) as Uint8Array<ArrayBuffer>);
    await new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
    conn.close();
    await expect(conn.transaction("kv", "readonly")).rejects.toMatchObject({ kind: "storage" });
  });

  it("names a full disk", () => {
    const e = storageError(new DOMException("quota", "QuotaExceededError"));
    expect(isHostError(e) && e.message).toContain("storage for this site is full");
  });
});
