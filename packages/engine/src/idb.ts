// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The engine's ordered key-value stores (`KvStore`, sync.md §12.4) for the browser.
//
// `KvBackend` is the interface the wasm host calls (crates/wasm/src/host.rs `JsKvBackend`):
// keys are the core's tuple-encoded byte strings (`store::key`), compared as unsigned bytes.
//
// - `IdbKv`: one IndexedDB database per store with one object store, `kv`, whose out-of-line
//   keys are the raw key bytes. IndexedDB orders binary keys byte-wise (unsigned, a prefix
//   first), exactly like `MemKv`'s `BTreeMap<Vec<u8>, _>` and SQLite's BLOB keys, so scans need
//   no re-encoding. Each `write` is one `readwrite` transaction (atomic, ops applied in
//   order), with durability `strict` for `state` and `relaxed` for `cache`. Forward scans use
//   `getAllKeys` + `getAll` over the same range in one transaction; reverse scans a `prev`
//   cursor.
// - `MemKv`: the same contract in memory (tests, previews, a failed IndexedDB open).
//
// Empty and inverted ranges select nothing (IndexedDB would throw `DataError`), and a limit
// of 0 returns nothing (IndexedDB's `getAll(range, 0)` would return everything). `IdbKv`
// refuses the empty key (some IndexedDB implementations take a zero-length buffer for a
// detached one); the core never writes it, every key starting with its tag.

import { hostError, isHostError } from "./protocol.ts";

/** The store interface of the wasm host. */
export interface KvBackend {
  /** One value (or `undefined`) per key, in request order. */
  getMany(keys: Uint8Array[]): Promise<(Uint8Array | undefined)[]>;
  /** At most `limit` pairs in range as a flat `[k, v, k, v, …]`, ascending (descending with
   * `reverse`). An `undefined` bound is unbounded; `…Open` excludes the bound. */
  scan(
    lower: Uint8Array | undefined,
    lowerOpen: boolean,
    upper: Uint8Array | undefined,
    upperOpen: boolean,
    reverse: boolean,
    limit: number,
  ): Promise<Uint8Array[]>;
  /** An atomic batch, flat: `0, key, value` put; `1, key` delete;
   * `2, lower, lowerOpen, upper, upperOpen` delete range. */
  write(ops: unknown[]): Promise<void>;
}

export const OP_PUT = 0;
export const OP_DELETE = 1;
export const OP_DELETE_RANGE = 2;

/** Unsigned byte-wise comparison (shorter prefix first). */
export function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    const d = (a[i] as number) - (b[i] as number);
    if (d !== 0) return d;
  }
  return a.length - b.length;
}

/** A key range with optional bounds. */
export interface Range {
  lower: Uint8Array | undefined;
  lowerOpen: boolean;
  upper: Uint8Array | undefined;
  upperOpen: boolean;
}

/** True if no key can be in `r`. */
export function rangeIsEmpty(r: Range): boolean {
  if (r.lower === undefined || r.upper === undefined) return false;
  const c = compareBytes(r.lower, r.upper);
  return c > 0 || (c === 0 && (r.lowerOpen || r.upperOpen));
}

export function inRange(k: Uint8Array, r: Range): boolean {
  if (r.lower !== undefined) {
    const c = compareBytes(k, r.lower);
    if (c < 0 || (c === 0 && r.lowerOpen)) return false;
  }
  if (r.upper !== undefined) {
    const c = compareBytes(k, r.upper);
    if (c > 0 || (c === 0 && r.upperOpen)) return false;
  }
  return true;
}

function asBytes(v: unknown, what: string): Uint8Array {
  if (v instanceof Uint8Array) return v;
  if (v instanceof ArrayBuffer) return new Uint8Array(v);
  throw hostError("storage", `${what} is not a byte string`);
}

/** Decodes the flat `write` array into ops. */
export type DecodedOp =
  | { op: "put"; key: Uint8Array; value: Uint8Array }
  | { op: "delete"; key: Uint8Array }
  | { op: "deleteRange"; range: Range };

export function decodeOps(ops: unknown[]): DecodedOp[] {
  const out: DecodedOp[] = [];
  let i = 0;
  const opt = (v: unknown): Uint8Array | undefined =>
    v === undefined || v === null ? undefined : asBytes(v, "a range bound");
  while (i < ops.length) {
    const tag = ops[i];
    if (tag === OP_PUT) {
      out.push({
        op: "put",
        key: asBytes(ops[i + 1], "a key"),
        value: asBytes(ops[i + 2], "a value"),
      });
      i += 3;
    } else if (tag === OP_DELETE) {
      out.push({ op: "delete", key: asBytes(ops[i + 1], "a key") });
      i += 2;
    } else if (tag === OP_DELETE_RANGE) {
      out.push({
        op: "deleteRange",
        range: {
          lower: opt(ops[i + 1]),
          lowerOpen: ops[i + 2] === true,
          upper: opt(ops[i + 3]),
          upperOpen: ops[i + 4] === true,
        },
      });
      i += 5;
    } else {
      throw hostError("storage", `unknown write op ${String(tag)}`);
    }
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// In memory
// ---------------------------------------------------------------------------------------------

const hex = (k: Uint8Array): string =>
  Array.from(k, (b) => b.toString(16).padStart(2, "0")).join("");

/** The `KvBackend` contract in memory (a sorted array of entries). */
export class MemKv implements KvBackend {
  private keys: Uint8Array[] = [];
  private readonly values = new Map<string, Uint8Array>();

  get size(): number {
    return this.keys.length;
  }

  private find(k: Uint8Array): { i: number; found: boolean } {
    let lo = 0;
    let hi = this.keys.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      const c = compareBytes(this.keys[mid] as Uint8Array, k);
      if (c === 0) return { i: mid, found: true };
      if (c < 0) lo = mid + 1;
      else hi = mid;
    }
    return { i: lo, found: false };
  }

  async getMany(keys: Uint8Array[]): Promise<(Uint8Array | undefined)[]> {
    return keys.map((k) => {
      const v = this.values.get(hex(k));
      return v === undefined ? undefined : v.slice();
    });
  }

  async scan(
    lower: Uint8Array | undefined,
    lowerOpen: boolean,
    upper: Uint8Array | undefined,
    upperOpen: boolean,
    reverse: boolean,
    limit: number,
  ): Promise<Uint8Array[]> {
    const r: Range = { lower, lowerOpen, upper, upperOpen };
    if (limit <= 0 || rangeIsEmpty(r)) return [];
    const hits = this.keys.filter((k) => inRange(k, r));
    if (reverse) hits.reverse();
    const out: Uint8Array[] = [];
    for (const k of hits.slice(0, limit)) {
      out.push(k.slice(), (this.values.get(hex(k)) as Uint8Array).slice());
    }
    return out;
  }

  async write(ops: unknown[]): Promise<void> {
    // decode everything first: a malformed batch changes nothing
    for (const op of decodeOps(ops)) {
      if (op.op === "put") {
        const { i, found } = this.find(op.key);
        if (!found) this.keys.splice(i, 0, op.key.slice());
        this.values.set(hex(op.key), op.value.slice());
      } else if (op.op === "delete") {
        const { i, found } = this.find(op.key);
        if (found) {
          this.keys.splice(i, 1);
          this.values.delete(hex(op.key));
        }
      } else if (!rangeIsEmpty(op.range)) {
        const range = op.range;
        this.keys = this.keys.filter((k) => {
          if (!inRange(k, range)) return true;
          this.values.delete(hex(k));
          return false;
        });
      }
    }
  }
}

// ---------------------------------------------------------------------------------------------
// IndexedDB
// ---------------------------------------------------------------------------------------------

const STORE = "kv";

/** A key as IndexedDB takes it (binary keys are `BufferSource`s). */
const idbKey = (k: Uint8Array): IDBValidKey => k as Uint8Array<ArrayBuffer>;

function req<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(storageError(r.error));
  });
}

function storageError(e: unknown): unknown {
  if (isHostError(e)) return e;
  const name = e instanceof DOMException ? e.name : "Error";
  const message = e instanceof Error ? e.message : String(e);
  return hostError("storage", `IndexedDB ${name}: ${message}`);
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(storageError(tx.error));
    tx.onabort = () => reject(storageError(tx.error ?? new DOMException("aborted", "AbortError")));
  });
}

/** `r` without zero-length bounds (no stored key is empty), or `null` if it selects nothing. */
function withoutEmptyBounds(r: Range): Range | null {
  if (r.upper !== undefined && r.upper.length === 0) return null;
  if (r.lower !== undefined && r.lower.length === 0)
    r = { ...r, lower: undefined, lowerOpen: false };
  return rangeIsEmpty(r) ? null : r;
}

/** An `IDBKeyRange` for a non-empty range without empty bounds (`undefined`: every key). */
function idbRange(f: IDBFactoryLike, r: Range): IDBKeyRange | undefined {
  const KR = f.keyRange;
  if (r.lower !== undefined && r.upper !== undefined) {
    return KR.bound(idbKey(r.lower), idbKey(r.upper), r.lowerOpen, r.upperOpen);
  }
  if (r.lower !== undefined) return KR.lowerBound(idbKey(r.lower), r.lowerOpen);
  if (r.upper !== undefined) return KR.upperBound(idbKey(r.upper), r.upperOpen);
  return undefined;
}

/** `indexedDB` and `IDBKeyRange` (globals in browsers; injectable for tests). */
export interface IDBFactoryLike {
  factory: IDBFactory;
  keyRange: typeof IDBKeyRange;
}

export function globalIdb(): IDBFactoryLike | null {
  const g = globalThis as { indexedDB?: IDBFactory; IDBKeyRange?: typeof IDBKeyRange };
  return g.indexedDB && g.IDBKeyRange ? { factory: g.indexedDB, keyRange: g.IDBKeyRange } : null;
}

export type Durability = "strict" | "relaxed";

/** A `KvBackend` over one IndexedDB database. */
export class IdbKv implements KvBackend {
  private constructor(
    private readonly db: IDBDatabase,
    private readonly idb: IDBFactoryLike,
    private readonly durability: Durability,
  ) {}

  /** Opens (creating if needed) the database `name`. */
  static async open(name: string, durability: Durability, idb = globalIdb()): Promise<IdbKv> {
    if (!idb) throw hostError("storage", "IndexedDB is not available");
    const open = idb.factory.open(name, 1);
    open.onupgradeneeded = () => {
      if (!open.result.objectStoreNames.contains(STORE)) open.result.createObjectStore(STORE);
    };
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      open.onsuccess = () => resolve(open.result);
      open.onerror = () => reject(storageError(open.error));
      open.onblocked = () => reject(hostError("storage", `IndexedDB ${name} is blocked`));
    });
    // another tab upgraded the schema: close so it can proceed; this worker restarts
    db.onversionchange = () => db.close();
    return new IdbKv(db, idb, durability);
  }

  close(): void {
    this.db.close();
  }

  async getMany(keys: Uint8Array[]): Promise<(Uint8Array | undefined)[]> {
    if (keys.length === 0) return [];
    try {
      const tx = this.db.transaction(STORE, "readonly");
      const store = tx.objectStore(STORE);
      const got = await Promise.all(
        keys.map((k) => (k.length === 0 ? undefined : req(store.get(idbKey(k))))),
      );
      return got.map((v) => (v === undefined ? undefined : asBytes(v, "a stored value")));
    } catch (e) {
      throw storageError(e);
    }
  }

  async scan(
    lower: Uint8Array | undefined,
    lowerOpen: boolean,
    upper: Uint8Array | undefined,
    upperOpen: boolean,
    reverse: boolean,
    limit: number,
  ): Promise<Uint8Array[]> {
    const r = withoutEmptyBounds({ lower, lowerOpen, upper, upperOpen });
    if (limit <= 0 || r === null) return [];
    try {
      const range = idbRange(this.idb, r);
      const tx = this.db.transaction(STORE, "readonly");
      const store = tx.objectStore(STORE);
      const out: Uint8Array[] = [];
      if (!reverse) {
        const [ks, vs] = await Promise.all([
          req(store.getAllKeys(range, limit)),
          req(store.getAll(range, limit)),
        ]);
        if (ks.length !== vs.length) throw hostError("storage", "getAllKeys/getAll disagree");
        ks.forEach((k, i) => {
          out.push(asBytes(k, "a stored key"), asBytes(vs[i], "a stored value"));
        });
        return out;
      }
      await new Promise<void>((resolve, reject) => {
        const c = store.openCursor(range, "prev");
        c.onerror = () => reject(storageError(c.error));
        c.onsuccess = () => {
          const cur = c.result;
          if (!cur) return resolve();
          out.push(asBytes(cur.key, "a stored key"), asBytes(cur.value, "a stored value"));
          if (out.length / 2 >= limit) return resolve();
          cur.continue();
        };
      });
      return out;
    } catch (e) {
      throw storageError(e);
    }
  }

  async write(ops: unknown[]): Promise<void> {
    const decoded = decodeOps(ops);
    if (decoded.length === 0) return;
    let tx: IDBTransaction;
    try {
      tx = this.db.transaction(STORE, "readwrite", { durability: this.durability });
    } catch (e) {
      throw storageError(e);
    }
    const finished = done(tx);
    try {
      const store = tx.objectStore(STORE);
      for (const op of decoded) {
        if (op.op === "put") {
          if (op.key.length === 0) throw hostError("storage", "the empty key is not a store key");
          store.put(op.value, idbKey(op.key));
        } else if (op.op === "delete") {
          if (op.key.length > 0) store.delete(idbKey(op.key));
        } else {
          const r = withoutEmptyBounds(op.range);
          if (r === null) continue;
          const range = idbRange(this.idb, r);
          if (range === undefined) store.clear();
          else store.delete(range);
        }
      }
    } catch (e) {
      // a request that throws synchronously (a bad key): nothing of the batch is applied
      try {
        tx.abort();
      } catch {
        // already finished
      }
      await finished.catch(() => {});
      throw storageError(e);
    }
    await finished;
  }
}
