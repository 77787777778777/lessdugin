// SPDX-License-Identifier: AGPL-3.0-or-later
// The JS host's `fetchBlob` runs one download per CID at a time (sync.md §12.2; review
// robustness-4): a view that stopped waiting for a spilled body (BODY_BLOB_WAIT_MS) left its
// download running, and every later view and retry started another one for the same CID.
import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import { MemoryBlobStore } from "./blobs.ts";
import { createJsHost } from "./host.ts";
import { MemKv } from "./idb.ts";
import { LocalLocks } from "./locks.ts";

function base32(bytes: Uint8Array): string {
  const A = "abcdefghijklmnopqrstuvwxyz234567";
  let bits = 0;
  let value = 0;
  let out = "";
  for (const b of bytes) {
    value = (value << 8) | b;
    bits += 8;
    while (bits >= 5) {
      out += A[(value >>> (bits - 5)) & 31];
      bits -= 5;
    }
  }
  if (bits > 0) out += A[(value << (5 - bits)) & 31];
  return out;
}

function rawCid(data: Uint8Array): string {
  const digest = createHash("sha256").update(data).digest();
  return `b${base32(Uint8Array.from([0x01, 0x55, 0x12, 0x20, ...digest]))}`;
}

function host(fetchFn: typeof fetch, blobs: MemoryBlobStore) {
  return createJsHost({
    cache: new MemKv(),
    state: new MemKv(),
    fetch: { fetch: fetchFn, policy: { devnet: false } },
    userPds: async () => {
      throw new Error("unused");
    },
    blobs,
    locks: new LocalLocks(),
  });
}

const PDS = "https://pds.example.com";
const DID = "did:plc:a";

describe("fetchBlob through the JS host", () => {
  it("runs one download per CID at a time, which a caller that stops waiting does not stop", async () => {
    const data = new TextEncoder().encode("a spilled body, longer than one chunk");
    const cid = rawCid(data);
    let requests = 0;
    let release: (() => void) | null = null;
    // the body comes in two chunks; the second one when the test releases it
    const fetchFn = (async () => {
      requests++;
      let sent = 0;
      const body = new ReadableStream<Uint8Array>({
        async pull(c) {
          if (sent === 0) {
            sent = 4;
            c.enqueue(data.slice(0, 4));
            return;
          }
          await new Promise<void>((r) => {
            release = r;
          });
          c.enqueue(data.slice(4));
          c.close();
        },
      });
      return new Response(body, { status: 200 });
    }) as typeof fetch;
    const blobs = new MemoryBlobStore();
    const h = host(fetchFn, blobs);
    const first = h.fetchBlob(PDS, DID, cid, 1024);
    // the core stops waiting after BODY_BLOB_WAIT_MS and asks again later: the same download
    const second = h.fetchBlob(PDS, DID, cid, 1024);
    expect(second).toBe(first);
    await new Promise((r) => setTimeout(r, 20));
    expect(requests).toBe(1);
    expect(await blobs.get(cid)).toBeUndefined();
    expect(release).not.toBeNull();
    (release as unknown as () => void)();
    const handle = await first;
    expect(handle).toEqual({ cid, size: data.length, locator: `mem:${cid}` });
    expect(await second).toEqual(handle);
    expect(requests).toBe(1);
    // done: the next call is the stored copy, without a request
    expect(await h.fetchBlob(PDS, DID, cid, 1024)).toEqual(handle);
    expect(requests).toBe(1);
  });

  it("forgets a download that failed, so the next call tries again", async () => {
    let requests = 0;
    const fetchFn = (async () => {
      requests++;
      return new Response("nope", { status: 500 });
    }) as typeof fetch;
    const h = host(fetchFn, new MemoryBlobStore());
    const cid = rawCid(new Uint8Array([1, 2, 3]));
    await expect(h.fetchBlob(PDS, DID, cid, 1024)).rejects.toMatchObject({ kind: "httpStatus" });
    await expect(h.fetchBlob(PDS, DID, cid, 1024)).rejects.toMatchObject({ kind: "httpStatus" });
    expect(requests).toBe(2);
  });
});
