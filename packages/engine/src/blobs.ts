// SPDX-License-Identifier: AGPL-3.0-or-later
//
// `Host::fetch_blob` for the browser (threat-model §5.6.1, sync.md §12.2): the blob is fetched
// from `<pds>/xrpc/com.atproto.sync.getBlob` without redirects, streamed in JS with the byte cap,
// assembled into one `Blob`, hashed with `crypto.subtle.digest("SHA-256", …)` and compared with
// the CID's multihash; only on a match is it stored (keyed by CID) and a handle returned. No
// blob byte enters wasm memory (INFRA-6). An already stored CID returns its handle without a
// request.

import { checkUrl, type FetchDeps, readCapped } from "./fetch.ts";
import { hostError, isHostError } from "./protocol.ts";

// ---------------------------------------------------------------------------------------------
// CIDs
// ---------------------------------------------------------------------------------------------

const B32 = "abcdefghijklmnopqrstuvwxyz234567";

/** RFC 4648 base32, lowercase, no padding. */
export function base32Decode(s: string): Uint8Array | null {
  const out: number[] = [];
  let bits = 0;
  let value = 0;
  for (const ch of s) {
    const v = B32.indexOf(ch);
    if (v < 0) return null;
    value = (value << 5) | v;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      out.push((value >> bits) & 0xff);
    }
  }
  // non-canonical trailing bits
  if (bits >= 5 || (value & ((1 << bits) - 1)) !== 0) return null;
  return Uint8Array.from(out);
}

function varint(b: Uint8Array, at: number): [number, number] | null {
  let v = 0;
  let shift = 0;
  for (let i = at; i < b.length && i < at + 9; i++) {
    const byte = b[i] as number;
    v += (byte & 0x7f) * 2 ** shift;
    shift += 7;
    if (byte < 0x80) return [v, i + 1];
  }
  return null;
}

/** The sha2-256 digest of a CIDv1 with the raw codec (`b…` base32), or `null` for any other
 * CID (atproto blobs are exactly these, data-model.md §2). */
export function rawSha256Digest(cid: string): Uint8Array | null {
  if (!cid.startsWith("b") || cid.length > 100) return null;
  const bytes = base32Decode(cid.slice(1));
  if (!bytes) return null;
  const ver = varint(bytes, 0);
  if (ver?.[0] !== 1) return null;
  const codec = varint(bytes, ver[1]);
  if (codec?.[0] !== 0x55) return null;
  const mh = varint(bytes, codec[1]);
  if (mh?.[0] !== 0x12) return null;
  const len = varint(bytes, mh[1]);
  if (len?.[0] !== 32 || bytes.length !== len[1] + 32) return null;
  return bytes.slice(len[1]);
}

// ---------------------------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------------------------

/** Where verified blobs live, keyed by CID. */
export interface BlobStore {
  get(cid: string): Promise<Blob | undefined>;
  put(cid: string, blob: Blob): Promise<void>;
  delete(cid: string): Promise<void>;
  /** The handle locator of a stored CID. */
  locator(cid: string): string;
}

export class MemoryBlobStore implements BlobStore {
  private readonly blobs = new Map<string, Blob>();

  async get(cid: string): Promise<Blob | undefined> {
    return this.blobs.get(cid);
  }

  async put(cid: string, blob: Blob): Promise<void> {
    this.blobs.set(cid, blob);
  }

  async delete(cid: string): Promise<void> {
    this.blobs.delete(cid);
  }

  locator(cid: string): string {
    return `mem:${cid}`;
  }
}

/** Verified blobs in an IndexedDB object store (`Blob` values; the browser keeps them on
 * disk). */
export class IdbBlobStore implements BlobStore {
  private constructor(private readonly db: IDBDatabase) {}

  static async open(name: string, factory: IDBFactory = indexedDB): Promise<IdbBlobStore> {
    const open = factory.open(name, 1);
    open.onupgradeneeded = () => {
      if (!open.result.objectStoreNames.contains("blobs")) open.result.createObjectStore("blobs");
    };
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      open.onsuccess = () => resolve(open.result);
      open.onerror = () => reject(hostError("storage", `IndexedDB ${name}: ${open.error}`));
    });
    db.onversionchange = () => db.close();
    return new IdbBlobStore(db);
  }

  close(): void {
    this.db.close();
  }

  private run<T>(mode: IDBTransactionMode, f: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
    return new Promise((resolve, reject) => {
      const r = f(this.db.transaction("blobs", mode).objectStore("blobs"));
      r.onsuccess = () => resolve(r.result);
      r.onerror = () => reject(hostError("storage", `blob store: ${r.error}`));
    });
  }

  async get(cid: string): Promise<Blob | undefined> {
    const v = await this.run("readonly", (s) => s.get(cid));
    return v instanceof Blob ? v : undefined;
  }

  async put(cid: string, blob: Blob): Promise<void> {
    await this.run("readwrite", (s) => s.put(blob, cid));
  }

  async delete(cid: string): Promise<void> {
    await this.run("readwrite", (s) => s.delete(cid));
  }

  locator(cid: string): string {
    return `idb:${cid}`;
  }
}

// ---------------------------------------------------------------------------------------------
// fetch_blob
// ---------------------------------------------------------------------------------------------

export interface BlobHandleValue {
  cid: string;
  size: number;
  locator: string;
}

function equal(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

/** `Host::fetch_blob(pds, did, cid, maxBytes)`. */
export async function fetchBlob(
  deps: FetchDeps & { store: BlobStore; timeoutMs?: number },
  pds: string,
  did: string,
  cid: string,
  maxBytes: number,
): Promise<BlobHandleValue> {
  const digest = rawSha256Digest(cid);
  if (!digest) throw hostError("unsupported", `${cid} is not a CIDv1 raw sha2-256 CID`);
  const stored = await deps.store.get(cid);
  if (stored) return { cid, size: stored.size, locator: deps.store.locator(cid) };
  const base = checkUrl(pds, deps.policy);
  const u = new URL("/xrpc/com.atproto.sync.getBlob", base);
  u.searchParams.set("did", did);
  u.searchParams.set("cid", cid);
  const ctl = new AbortController();
  const timer =
    deps.timeoutMs !== undefined ? setTimeout(() => ctl.abort(), deps.timeoutMs) : undefined;
  try {
    let resp: Response;
    try {
      resp = await deps.fetch(u.href, {
        cache: "no-store",
        credentials: "omit",
        mode: "cors",
        referrerPolicy: "no-referrer",
        redirect: "manual",
        signal: ctl.signal,
      });
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") {
        throw hostError("timeout", `getBlob from ${u.host} timed out`);
      }
      throw hostError("network", `getBlob from ${u.host}: ${e instanceof Error ? e.message : e}`);
    }
    if (resp.type === "opaqueredirect") {
      throw hostError("blocked", `getBlob from ${u.host} redirected (not followed)`);
    }
    if (resp.status < 200 || resp.status > 299) {
      let name: string | undefined;
      try {
        const body = await readCapped(resp.body, 4096);
        const j = JSON.parse(new TextDecoder().decode(body)) as { error?: unknown };
        if (typeof j.error === "string") name = j.error;
      } catch {
        // no XRPC error name
      }
      throw hostError("httpStatus", `getBlob: HTTP ${resp.status}`, {
        status: resp.status,
        ...(name !== undefined ? { name } : {}),
      });
    }
    let bytes: Uint8Array;
    try {
      bytes = await readCapped(resp.body, maxBytes);
    } catch (e) {
      if (isHostError(e)) throw e;
      throw hostError("network", `getBlob from ${u.host}: ${e instanceof Error ? e.message : e}`);
    }
    const blob = new Blob([bytes as BlobPart]);
    const got = new Uint8Array(await crypto.subtle.digest("SHA-256", await blob.arrayBuffer()));
    if (!equal(got, digest)) throw hostError("cidMismatch", `${cid}: the bytes do not match`);
    await deps.store.put(cid, blob);
    return { cid, size: blob.size, locator: deps.store.locator(cid) };
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
